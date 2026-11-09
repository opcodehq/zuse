//! Snapshot preparation from a clean managed base; never snapshots a task VM.
use super::source::Bundles;
use super::worker::{
    Operation, PreparationBackend, PreparationPlan, Request, StepMode, StepResult,
};
use super::*;
use crate::providers::{
    MachineState,
    boxd::{Boxd, SnapshotState},
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct BoxdPreparation {
    pool: PgPool,
    provider: Arc<Boxd>,
    bundles: Arc<dyn Bundles>,
}
fn error(_: crate::providers::ProviderError) -> ApiError {
    ApiError::unavailable("project_cache_provider_outcome_uncertain")
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn names(op: &Operation) -> (String, String, String) {
    let id = op
        .operation_id
        .strip_prefix("cacheop_")
        .unwrap_or(&op.operation_id);
    (
        format!("pc-b-{id}"),
        format!("pc-v-{id}"),
        format!("pc-s-{id}"),
    )
}
fn expected(op: &Operation) -> Result<PreparationReceipt, ApiError> {
    Ok(PreparationReceipt {
        version: 1,
        cache_key: op.plan.identity.key()?,
        operation_id: op.operation_id.clone(),
        runtime_sha256: op.plan.identity.base_runtime_sha256.clone(),
        recipe_digest: op.plan.identity.recipe_digest.clone(),
        dependency_digest: op.plan.identity.dependency_digest(),
        source_commit: op.plan.identity.source_commit.clone(),
        source_bundle_sha256: op.plan.source_bundle_sha256.clone(),
        credentials_removed: true,
        task_storage_absent: true,
        agent_profiles_absent: true,
        secret_free_source: true,
    })
}
fn guest_plan(op: &Operation) -> Result<zuse_cloud_runtime::project_cache::Plan, ApiError> {
    Ok(zuse_cloud_runtime::project_cache::Plan {
        version: 1,
        cache_key: op.plan.identity.key()?,
        operation_id: op.operation_id.clone(),
        identity: json!(op.plan.identity),
        recipe: op.plan.recipe.clone(),
        source_bundle_sha256: op.plan.source_bundle_sha256.clone(),
        home: "/home/zuse".into(),
        cache_root: zuse_cloud_runtime::project_cache::ROOT.into(),
        bundle: "/opt/zuse/preparation/source.bundle".into(),
        supervisor: true,
    })
}
fn capture_receipt(op: &Operation, value: &Value) -> Result<(), ApiError> {
    let proof = &value["receipt"];
    let hygiene = &proof["hygiene"];
    if value["state"] != "ready"
        || proof["version"] != 1
        || proof["preparationCaptureVersion"] != 1
        || proof["cacheKey"] != op.plan.identity.key()?
        || proof["operationId"] != op.operation_id
        || proof["runtimeSha256"] != op.plan.identity.base_runtime_sha256
        || !proof["preparedReceiptSha256"].as_str().is_some_and(|v| {
            v.len() == 64
                && v.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        || hygiene["operationId"] != op.operation_id
        || hygiene["runtimeSha256"] != op.plan.identity.base_runtime_sha256
        || hygiene["atomicStopAcknowledged"] != true
        || hygiene["legacyBuilder"] != false
        || hygiene["runtimeDataAbsent"] != true
        || hygiene["originalResidentExited"] != true
        || hygiene["supervisorLockReleased"] != true
        || hygiene["enabledBootServiceRetained"] != true
    {
        return Err(ApiError::conflict("project_cache_capture_hygiene_required"));
    }
    Ok(())
}
impl BoxdPreparation {
    pub fn new(pool: PgPool, provider: Arc<Boxd>, bundles: Arc<dyn Bundles>) -> Self {
        Self {
            pool,
            provider,
            bundles,
        }
    }
    async fn pilot_effect_allowed(&self, op: &Operation) -> Result<(), ApiError> {
        if op.plan.identity.staging_pilot_sha256.is_some() {
            let mut tx = self.pool.begin().await?;
            let allowed = super::pilot::authorized_tx(
                &mut tx,
                &op.plan.identity,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
            tx.commit().await?;
            if !allowed {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
        }
        Ok(())
    }
    async fn create_budget(&self, op: &Operation) -> Result<(), ApiError> {
        if op.plan.identity.staging_pilot_sha256.is_some() {
            let mut tx = self.pool.begin().await?;
            if !super::pilot::authorized_tx(
                &mut tx,
                &op.plan.identity,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?
            {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
            super::pilot::capacity_tx(&mut tx, Some(&op.operation_id)).await?;
            tx.commit().await?;
        }
        Ok(())
    }
    async fn exec(&self, id: &str, command: &str) -> Result<Value, ApiError> {
        let result = self.provider.exec(id, command).await.map_err(error)?;
        if result.exit_code != 0 || result.stdout.len() > 1024 * 1024 {
            return Err(ApiError::conflict("project_cache_guest_not_ready"));
        }
        serde_json::from_slice(&result.stdout)
            .map_err(|_| ApiError::conflict("project_cache_guest_receipt_invalid"))
    }
    async fn capability(&self, id: &str, op: &Operation) -> Result<(), ApiError> {
        let receipt=self.exec(id,"sudo -n -u zuse env -i HOME=/home/zuse PATH=/opt/zuse/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/zuse-cloud-runtime project-cache-capabilities").await?;
        let receipt: CacheCapabilityReceipt = serde_json::from_value(receipt)
            .map_err(|_| ApiError::conflict("project_cache_runtime_update_required"))?;
        if !receipt.valid(&op.plan.identity.base_runtime_sha256) {
            return Err(ApiError::conflict("project_cache_runtime_update_required"));
        }
        Ok(())
    }
    async fn stamp(&self, id: &str, op: &Operation) -> Result<Value, ApiError> {
        let root = format!("/opt/zuse/project-caches/{}", op.plan.identity.key()?);
        let script = r#"import json,os,sys
p=sys.argv[1];out={'receipt':None,'intent':False,'failed':False}
f=p+'/receipt.json'
if os.path.isfile(f) and not os.path.islink(f) and os.stat(f).st_size<=1048576:
 out['receipt']=json.load(open(f))
out['intent']=os.path.exists(p+'/preparation-intent.json')
f=p+'/preparation/environment-ready.json'
if os.path.isfile(f) and not os.path.islink(f) and os.stat(f).st_size<=1048576:
 out['failed']=json.load(open(f)).get('state') in ('failed','uncertain')
print(json.dumps(out,separators=(',',':')))
"#;
        self.exec(
            id,
            &format!("python3 -I -c {} {}", quote(script), quote(&root)),
        )
        .await
    }
    async fn bootstrap(
        &self,
        id: &str,
        name: &str,
        artifact: &str,
        op: &Operation,
    ) -> Result<bool, ApiError> {
        use crate::managed_bases::bootstrap::{Purpose, Request};
        self.pilot_effect_allowed(op).await?;
        let spec = op
            .plan
            .initial_runtime_bootstrap
            .as_ref()
            .ok_or_else(|| ApiError::conflict("project_cache_initial_bootstrap_required"))?;
        if !spec.valid(op.plan.identity.provider) {
            return Err(ApiError::conflict(
                "project_cache_initial_bootstrap_invalid",
            ));
        }
        let request = Request {
            version: 1,
            provider: op.plan.identity.provider,
            purpose: Purpose::CachePreparation,
            operation_id: op.operation_id.clone(),
            workspace_id: format!("cache:{}", op.plan.identity.key()?),
            generation: 1,
            storage_incarnation_id: format!("cache:{}", op.operation_id),
            source_machine_id: id.into(),
            image_pins: spec.pins(),
        };
        self.provider
            .verify_initial_snapshot_source(id, artifact, name)
            .await
            .map_err(error)?;
        let result = self
            .provider
            .exec(id, &request.command().map_err(error)?)
            .await
            .map_err(error)?;
        if result.exit_code != 0 || result.stdout.len() > 65536 {
            return Err(ApiError::conflict(
                "project_cache_initial_bootstrap_rejected",
            ));
        }
        let value: Value = serde_json::from_slice(&result.stdout)
            .map_err(|_| ApiError::conflict("project_cache_initial_bootstrap_invalid"))?;
        Ok(request.response(&value).map_err(error)?.is_some())
    }
    async fn capture_state(
        &self,
        id: &str,
        op: &Operation,
        status: bool,
    ) -> Result<Value, ApiError> {
        let plan = serde_json::to_vec(&guest_plan(op)?)
            .map_err(|_| ApiError::conflict("project_cache_plan_invalid"))?;
        self.exec(id,&format!("env -i HOME=/home/zuse PATH=/usr/bin:/bin /usr/local/bin/zuse-cloud-runtime capture-project-cache --plan /opt/zuse/preparation/project-cache.json --plan-sha256 {:x}{}",Sha256::digest(&plan),if status {" --status"} else {""})).await
    }
    async fn retire(&self, id: Option<&str>, name: &str) -> Result<bool, ApiError> {
        let resource = self
            .provider
            .inspect(id.unwrap_or(name))
            .await
            .map_err(error)?;
        let Some(resource) = resource else {
            return Ok(true);
        };
        if resource.name != name || id.is_some_and(|id| id != resource.id) {
            return Err(ApiError::conflict(
                "project_cache_resource_identity_changed",
            ));
        }
        self.provider.destroy(&resource.id).await.map_err(error)?;
        Ok(self
            .provider
            .inspect(&resource.id)
            .await
            .map_err(error)?
            .is_none())
    }
}
#[async_trait]
impl PreparationBackend for BoxdPreparation {
    async fn reap_source(&self, now: i64) -> Result<bool, ApiError> {
        self.bundles.reap(now).await
    }
    async fn resolve(&self, request: &Request) -> Result<Vec<PreparationPlan>, ApiError> {
        let now = chrono::Utc::now().timestamp_millis();
        if let Some(authority) = super::pilot::current(now)? {
            let mut tx = self.pool.begin().await?;
            authority.request_tx(&mut tx, request, now).await?;
            let candidate = authority.candidate_tx(&mut tx, now).await?;
            tx.commit().await?;
            let bundle = self.bundles.resolve(request).await?;
            let identity = CacheIdentity {
                version: 1,
                owner_id: request.owner.clone(),
                project_id: request.project.clone(),
                repository_identity: request.repository_identity.clone(),
                provider: zuse_protocol::cloud_computers::ProviderId::Boxd,
                architecture: candidate.architecture.clone(),
                base_release_id: candidate.release_id.clone(),
                base_artifact_id: candidate.artifact_id.clone(),
                base_artifact_version: Some(authority.pilot.base_artifact_version.clone()),
                base_runtime_sha256: candidate.runtime_sha256.clone(),
                recipe_source: bundle.recipe_source.clone(),
                recipe_digest: format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&bundle.recipe)
                            .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?
                    )
                ),
                source_commit: bundle.commit.clone(),
                dependency_inputs: bundle.dependency_inputs.clone(),
                staging_pilot_sha256: Some(authority.pilot.fingerprint()?),
            };
            if !authority.pilot.permits(&identity)? {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
            let original = self
                .provider
                .inspect_snapshot(&candidate.artifact_id)
                .await
                .map_err(error)?
                .ok_or_else(|| ApiError::conflict("project_cache_managed_base_missing"))?;
            if original.id != candidate.artifact_id
                || original.state != SnapshotState::Ready
                || original.version.to_string() != authority.pilot.base_artifact_version
            {
                return Err(ApiError::conflict("staging_project_cache_pilot_fenced"));
            }
            return Ok(vec![PreparationPlan {
                identity,
                recipe: bundle.recipe,
                base_artifact_binding: candidate.provider_artifact_binding,
                initial_runtime_bootstrap: candidate.initial_runtime_bootstrap,
                source_bundle_sha256: bundle.sha256,
            }]);
        }
        let rows:Vec<(String,String,String,Value,Value)>=sqlx::query_as("SELECT b.release_id,b.artifact_id,b.architecture,b.runtime_manifest,c.receipt FROM cloud_managed_bases b JOIN cloud_project_cache_capabilities c USING(provider,release_id) WHERE b.provider='boxd' AND b.state='verified' AND c.state='certified' ORDER BY b.created_at DESC,b.release_id DESC LIMIT 1").fetch_all(&self.pool).await?;
        if rows.is_empty() {
            return Ok(vec![]);
        }
        let bundle = self.bundles.resolve(request).await?;
        let mut plans = Vec::new();
        for (release_id, artifact, architecture, manifest, cap) in rows {
            let release: crate::managed_bases::ManagedBaseRelease =
                serde_json::from_value(manifest)
                    .map_err(|_| ApiError::conflict("project_cache_managed_base_invalid"))?;
            release.validate()?;
            if release.provider != zuse_protocol::cloud_computers::ProviderId::Boxd
                || release.release_id != release_id
                || release.artifact_id != artifact
                || release.architecture != architecture
            {
                return Err(ApiError::conflict("project_cache_managed_base_invalid"));
            }

            let cap: CacheCapabilityReceipt = serde_json::from_value(cap)
                .map_err(|_| ApiError::conflict("project_cache_runtime_capability_required"))?;
            if !cap.valid(&cap.runtime_sha256) {
                return Err(ApiError::conflict(
                    "project_cache_runtime_capability_required",
                ));
            }
            if release
                .manifest
                .command_admission
                .as_ref()
                .is_some_and(|r| r.runtime_sha256 != cap.runtime_sha256)
            {
                return Err(ApiError::conflict(
                    "project_cache_managed_base_runtime_mismatch",
                ));
            }

            let Some(initial_bootstrap) = release.initial_runtime_bootstrap.clone() else {
                continue;
            };
            if !initial_bootstrap.valid(zuse_protocol::cloud_computers::ProviderId::Boxd)
                || initial_bootstrap.runtime_sha256 != cap.runtime_sha256
            {
                continue;
            }
            let source = self
                .provider
                .inspect_snapshot(&artifact)
                .await
                .map_err(error)?
                .ok_or_else(|| ApiError::conflict("project_cache_managed_base_missing"))?;
            if source.id != artifact || source.state != SnapshotState::Ready || source.version == 0
            {
                return Err(ApiError::conflict("project_cache_managed_base_invalid"));
            }
            let identity = CacheIdentity {
                version: 1,
                owner_id: request.owner.clone(),
                project_id: request.project.clone(),
                repository_identity: request.repository_identity.clone(),
                provider: zuse_protocol::cloud_computers::ProviderId::Boxd,
                architecture,
                base_release_id: release_id,
                base_artifact_id: artifact,
                base_artifact_version: Some(source.version.to_string()),
                base_runtime_sha256: cap.runtime_sha256,
                recipe_source: bundle.recipe_source.clone(),
                recipe_digest: format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&bundle.recipe)
                            .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?
                    )
                ),
                source_commit: bundle.commit.clone(),
                dependency_inputs: bundle.dependency_inputs.clone(),
                staging_pilot_sha256: None,
            };
            plans.push(PreparationPlan {
                identity,
                recipe: bundle.recipe.clone(),
                base_artifact_binding: None,
                initial_runtime_bootstrap: Some(initial_bootstrap),
                source_bundle_sha256: bundle.sha256.clone(),
            });
        }
        Ok(plans)
    }
    async fn step(&self, op: &Operation, mode: StepMode) -> Result<StepResult, ApiError> {
        if op.plan.identity.provider != zuse_protocol::cloud_computers::ProviderId::Boxd {
            return Err(ApiError::conflict("project_cache_provider_not_certified"));
        }
        if op.plan.identity.staging_pilot_sha256.is_some()
            && !matches!(op.phase.as_str(), "cleanup" | "complete")
        {
            let mut tx = self.pool.begin().await?;
            if !super::pilot::authorized_tx(
                &mut tx,
                &op.plan.identity,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?
            {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
            tx.commit().await?;
        }
        let (builder_name, verifier_name, snapshot_name) = names(op);
        let mut resources = op.resources["provider"]
            .as_object()
            .cloned()
            .map(Value::Object)
            .unwrap_or_else(|| json!({}));
        let version = op
            .plan
            .identity
            .base_artifact_version
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| ApiError::conflict("project_cache_managed_base_version_required"))?;
        let advance = |phase: &str, resources: Value| {
            Ok(StepResult::Advance {
                phase: phase.into(),
                resources,
            })
        };
        if op.plan.initial_runtime_bootstrap.is_none()
            && !matches!(op.phase.as_str(), "cleanup" | "complete")
        {
            return Ok(StepResult::Failed {
                code: "project_cache_initial_bootstrap_required",
                resources,
            });
        }
        match op.phase.as_str() {
            "allocate" => {
                let machine = self
                    .provider
                    .inspect(resources["builderId"].as_str().unwrap_or(&builder_name))
                    .await
                    .map_err(error)?;
                let machine = match machine {
                    Some(m) => m,
                    None if mode == StepMode::Begin => {
                        if let Err(error) = self.create_budget(op).await {
                            resources["builderNotSubmitted"] = json!(true);
                            return Ok(StepResult::Failed {
                                code: error.code,
                                resources,
                            });
                        }
                        self.provider
                            .create_from_snapshot(
                                &op.plan.identity.base_artifact_id,
                                &builder_name,
                                true,
                            )
                            .await
                            .map_err(error)?
                    }
                    None => return Ok(StepResult::Pending(resources)),
                };
                if machine.name != builder_name
                    || resources["builderId"]
                        .as_str()
                        .is_some_and(|id| id != machine.id)
                {
                    return Err(ApiError::conflict(
                        "project_cache_resource_identity_changed",
                    ));
                }
                resources["builderId"] = json!(machine.id);
                if machine.state != MachineState::Running {
                    return Ok(StepResult::Pending(resources));
                }
                self.provider
                    .verify_snapshot_source(
                        &machine.id,
                        &op.plan.identity.base_artifact_id,
                        version,
                    )
                    .await
                    .map_err(error)?;
                self.capability(&machine.id, op).await?;
                if !self
                    .bootstrap(
                        &machine.id,
                        &builder_name,
                        &op.plan.identity.base_artifact_id,
                        op,
                    )
                    .await?
                {
                    return Ok(StepResult::Pending(resources));
                }
                advance("prepare", resources)
            }
            "prepare" => {
                let id = resources["builderId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("project_cache_builder_missing"))?;
                let stamp = self.stamp(id, op).await?;
                if !stamp["receipt"].is_null() {
                    if stamp["receipt"]["identity"] != json!(op.plan.identity)
                        || stamp["receipt"]["receipt"] != json!(expected(op)?)
                    {
                        return Err(ApiError::conflict("project_cache_guest_receipt_invalid"));
                    }
                    return advance("quiesce", resources);
                }
                if stamp["failed"] == true {
                    return Ok(StepResult::Failed {
                        code: "project_cache_environment_failed",
                        resources,
                    });
                }
                if stamp["intent"] == true {
                    return Ok(StepResult::Pending(resources));
                }
                // Reissuing the original guest admission is safe ONLY with its immutable
                // operationID and durable local intent+DataLock. It cannot rerun setup.
                let bundle = self
                    .bundles
                    .load(
                        &op.plan.source_bundle_sha256,
                        &op.request.owner,
                        &op.request.project,
                    )
                    .await?;
                self.bundles.authorize_placement(&op.request).await?;
                let mkdir=self.provider.exec(id,"install -d -m 700 -o zuse -g zuse /opt/zuse/project-caches /opt/zuse/preparation").await.map_err(error)?;
                if mkdir.exit_code != 0 {
                    return Err(ApiError::conflict("project_cache_upload_failed"));
                }
                for (n, chunk) in bundle.chunks(2 * 1024 * 1024).enumerate() {
                    self.bundles.authorize_placement(&op.request).await?;
                    self.provider
                        .upload(
                            id,
                            &format!("/opt/zuse/preparation/source.chunk.{n}"),
                            chunk,
                        )
                        .await
                        .map_err(error)?;
                }
                let paths = (0..bundle.chunks(2 * 1024 * 1024).count())
                    .map(|n| format!("/opt/zuse/preparation/source.chunk.{n}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let join=self.provider.exec(id,&format!("cat {paths} > /opt/zuse/preparation/source.bundle && chown zuse:zuse /opt/zuse/preparation/source.bundle && chmod 600 /opt/zuse/preparation/source.bundle")).await.map_err(error)?;
                if join.exit_code != 0 {
                    return Err(ApiError::conflict("project_cache_upload_failed"));
                }
                self.bundles.authorize_placement(&op.request).await?;
                let plan = guest_plan(op)?;
                self.provider
                    .upload(
                        id,
                        "/opt/zuse/preparation/project-cache.json",
                        &serde_json::to_vec(&plan)
                            .map_err(|_| ApiError::conflict("project_cache_plan_invalid"))?,
                    )
                    .await
                    .map_err(error)?;
                let command = "chown zuse:zuse /opt/zuse/preparation/project-cache.json && chmod 600 /opt/zuse/preparation/project-cache.json && sudo -n -u zuse env -i HOME=/home/zuse PATH=/opt/zuse/bin:/usr/local/bin:/usr/bin:/bin /usr/local/bin/zuse-cloud-runtime prepare-project-cache --plan /opt/zuse/preparation/project-cache.json";
                self.pilot_effect_allowed(op).await?;
                let result = self.provider.exec(id, command).await.map_err(error)?;
                if result.exit_code != 0 {
                    return Ok(StepResult::Failed {
                        code: "project_cache_environment_failed",
                        resources,
                    });
                }
                let stamp = self.stamp(id, op).await?;
                if stamp["receipt"]["receipt"] != json!(expected(op)?) {
                    return Err(ApiError::conflict("project_cache_guest_receipt_invalid"));
                }
                advance("quiesce", resources)
            }
            "quiesce" => {
                let id = resources["builderId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("project_cache_builder_missing"))?;
                let status = self.capture_state(id, op, true).await?;
                let status = if status["state"] == "not-started" {
                    self.capture_state(id, op, false).await?
                } else {
                    status
                };
                match status["state"].as_str() {
                    Some("ready") => {
                        capture_receipt(op, &status)?;
                        advance("capture", resources)
                    }
                    Some("failed") => Ok(StepResult::Failed {
                        code: "project_cache_capture_hygiene_failed",
                        resources,
                    }),
                    Some("uncertain") => Ok(StepResult::Pending(resources)),
                    _ => Err(ApiError::conflict("project_cache_capture_receipt_invalid")),
                }
            }
            "capture" => {
                let snapshot = self
                    .provider
                    .inspect_snapshot(&snapshot_name)
                    .await
                    .map_err(error)?;
                let snapshot = match snapshot {
                    Some(s) => s,
                    None if mode == StepMode::Begin => {
                        let id = resources["builderId"]
                            .as_str()
                            .ok_or_else(|| ApiError::conflict("project_cache_builder_missing"))?;
                        let proof = self.capture_state(id, op, true).await;
                        if match proof {
                            Ok(ref value) => capture_receipt(op, value).is_err(),
                            Err(_) => true,
                        } {
                            resources["captureNotSubmitted"] = json!(true);
                            return Ok(StepResult::Failed {
                                code: "project_cache_capture_hygiene_required",
                                resources,
                            });
                        }
                        if let Err(error) = self.pilot_effect_allowed(op).await {
                            resources["captureNotSubmitted"] = json!(true);
                            return Ok(StepResult::Failed {
                                code: error.code,
                                resources,
                            });
                        }
                        let s = self
                            .provider
                            .create_snapshot(id, &snapshot_name)
                            .await
                            .map_err(error)?;
                        resources["artifactId"] = json!(s.id);
                        resources["artifactVersion"] = json!(s.version.to_string());
                        return Ok(StepResult::Pending(resources));
                    }
                    None => return Ok(StepResult::Pending(resources)),
                };
                if snapshot.name != snapshot_name
                    || resources["artifactId"]
                        .as_str()
                        .is_some_and(|id| id != snapshot.id)
                    || resources["artifactVersion"]
                        .as_str()
                        .is_some_and(|v| v != snapshot.version.to_string())
                {
                    return Err(ApiError::conflict(
                        "project_cache_artifact_identity_changed",
                    ));
                }
                resources["artifactId"] = json!(snapshot.id);
                resources["artifactVersion"] = json!(snapshot.version.to_string());
                if snapshot.state == SnapshotState::Failed {
                    return Ok(StepResult::Failed {
                        code: "project_cache_capture_failed",
                        resources,
                    });
                }
                if snapshot.state != SnapshotState::Ready {
                    return Ok(StepResult::Pending(resources));
                }
                advance("retire-builder", resources)
            }
            "retire-builder" => {
                if !self
                    .retire(resources["builderId"].as_str(), &builder_name)
                    .await?
                {
                    return Ok(StepResult::Pending(resources));
                }
                resources["builderAbsent"] = json!(true);
                advance("verify", resources)
            }
            "verify" => {
                let artifact = resources["artifactId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("project_cache_artifact_missing"))?
                    .to_owned();
                let version = resources["artifactVersion"]
                    .as_str()
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or_else(|| ApiError::conflict("project_cache_artifact_missing"))?;
                let machine = self
                    .provider
                    .inspect(resources["verifierId"].as_str().unwrap_or(&verifier_name))
                    .await
                    .map_err(error)?;
                let machine = match machine {
                    Some(m) => m,
                    None if mode == StepMode::Begin => {
                        if let Err(error) = self.create_budget(op).await {
                            resources["verifierNotSubmitted"] = json!(true);
                            return Ok(StepResult::Failed {
                                code: error.code,
                                resources,
                            });
                        }
                        self.provider
                            .create_from_snapshot(&artifact, &verifier_name, true)
                            .await
                            .map_err(error)?
                    }
                    None => return Ok(StepResult::Pending(resources)),
                };
                if machine.name != verifier_name
                    || resources["verifierId"]
                        .as_str()
                        .is_some_and(|id| id != machine.id)
                {
                    return Err(ApiError::conflict(
                        "project_cache_resource_identity_changed",
                    ));
                }
                resources["verifierId"] = json!(machine.id);
                if machine.state != MachineState::Running {
                    return Ok(StepResult::Pending(resources));
                }
                self.provider
                    .verify_snapshot_source(&machine.id, &artifact, version)
                    .await
                    .map_err(error)?;
                self.capability(&machine.id, op).await?;
                if !self
                    .bootstrap(&machine.id, &verifier_name, &artifact, op)
                    .await?
                {
                    return Ok(StepResult::Pending(resources));
                }
                capture_receipt(op, &self.capture_state(&machine.id, op, true).await?)?;
                let stamp = self.stamp(&machine.id, op).await?;
                if stamp["receipt"]["identity"] != json!(op.plan.identity)
                    || stamp["receipt"]["receipt"] != json!(expected(op)?)
                {
                    return Err(ApiError::conflict(
                        "project_cache_snapshot_readback_mismatch",
                    ));
                }
                advance("retire-verifier", resources)
            }
            "retire-verifier" => {
                if !self
                    .retire(resources["verifierId"].as_str(), &verifier_name)
                    .await?
                {
                    return Ok(StepResult::Pending(resources));
                }
                resources["verifierAbsent"] = json!(true);
                advance("publish", resources)
            }
            "publish" => Ok(StepResult::Ready {
                artifact_id: resources["artifactId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("project_cache_artifact_missing"))?
                    .into(),
                artifact_version: resources["artifactVersion"].as_str().map(str::to_owned),
                artifact_binding: None,
                receipt: expected(op)?,
                resources,
            }),
            "cleanup" => {
                for (field, name, effect) in [
                    ("builderId", &builder_name, "allocate"),
                    ("verifierId", &verifier_name, "verify"),
                ] {
                    if resources[field].is_null() {
                        if let Some(machine) = self.provider.inspect(name).await.map_err(error)? {
                            if machine.name != *name {
                                return Err(ApiError::conflict(
                                    "project_cache_resource_identity_changed",
                                ));
                            }
                            resources[field] = json!(machine.id);
                            return Ok(StepResult::Pending(resources));
                        } else if op.resources["effects"][effect]["requested"] == true
                            && resources[if effect == "allocate" {
                                "builderNotSubmitted"
                            } else {
                                "verifierNotSubmitted"
                            }] != true
                        {
                            return Ok(StepResult::Pending(resources));
                        }
                    }
                }
                if !self
                    .retire(resources["builderId"].as_str(), &builder_name)
                    .await?
                    || !self
                        .retire(resources["verifierId"].as_str(), &verifier_name)
                        .await?
                {
                    return Ok(StepResult::Pending(resources));
                }
                resources["builderAbsent"] = json!(true);
                resources["verifierAbsent"] = json!(true);
                if let Some(snapshot) = self
                    .provider
                    .inspect_snapshot(&snapshot_name)
                    .await
                    .map_err(error)?
                {
                    if resources["artifactId"].is_null() {
                        resources["artifactId"] = json!(snapshot.id);
                        resources["artifactVersion"] = json!(snapshot.version.to_string());
                        return Ok(StepResult::Pending(resources));
                    }
                    if snapshot.name != snapshot_name
                        || snapshot.id == op.plan.identity.base_artifact_id
                        || resources["artifactId"]
                            .as_str()
                            .is_some_and(|id| id != snapshot.id)
                    {
                        return Err(ApiError::conflict(
                            "project_cache_artifact_identity_changed",
                        ));
                    }
                    self.provider
                        .delete_snapshot(&snapshot.id)
                        .await
                        .map_err(error)?;
                    return Ok(StepResult::Pending(resources));
                }
                if op.resources["effects"]["capture"]["requested"] == true
                    && resources["artifactId"].is_null()
                    && resources["captureNotSubmitted"] != true
                {
                    return Ok(StepResult::Pending(resources));
                }
                resources["unpublishedArtifactAbsent"] = json!(true);
                advance("complete", resources)
            }
            _ => Err(ApiError::conflict("project_cache_phase_invalid")),
        }
    }
}

#[cfg(test)]
mod transport_tests;

#[cfg(test)]
pub(super) use transport_tests::exercise_staging_pilot;
