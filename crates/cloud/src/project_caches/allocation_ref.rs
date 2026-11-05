//! Resolve outside SQL locks, then fence/persist the original allocation source.
use super::*;
use crate::jobs;
impl crate::runtime::RuntimeControl {
    pub(crate) async fn resolve_project_allocation_source(
        &self,
        job: &jobs::Job,
        workspace: &mut Value,
        operation: &mut Value,
    ) -> Result<bool, ApiError> {
        self.resolve_project_allocation_source_at(
            job,
            workspace,
            operation,
            "https://api.github.com",
        )
        .await
    }
    pub(super) async fn resolve_project_allocation_source_at(
        &self,
        job: &jobs::Job,
        workspace: &mut Value,
        operation: &mut Value,
        origin: &str,
    ) -> Result<bool, ApiError> {
        let config = &workspace["request_config"];
        let cached = !config["rustProjectCache"].is_null();
        if operation["phase"] != "planned"
            || operation["machine_id"].is_string()
            || config["managedReleaseId"].as_str().is_none()
            || (!config["managedCandidateSha256"].is_null()
                && config["stagingProjectCachePilotSha256"].is_null())
            || (!cached && !self.project_cache_control.configured())
            || config["workloadSecretBindings"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        {
            return Ok(false);
        }
        if !self.project_cache_control.available().await? {
            return Ok(false);
        }
        let owner = workspace["account_id"].as_str().unwrap_or_default();
        let project = workspace["project_id"].as_str().unwrap_or_default();
        let metadata:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE account_id=$1 AND project_id=$2 AND included=true")
            .bind(owner).bind(project).fetch_optional(&self.shared_pool()).await?;
        let Some(metadata) = metadata else {
            return Err(ApiError::conflict("project_cache_project_unavailable"));
        };
        let repository = metadata["repository_identity"].as_str().unwrap_or_default();
        let reference = config["rustResolvedSource"]["commit"]
            .as_str()
            .or_else(|| workspace["base_ref"].as_str())
            .unwrap_or_default();
        let recipe = serde_json::from_value(config["environmentRecipe"].clone());
        let resolved = if metadata["visibility"] == "public" {
            match recipe {
                Ok(recipe) => {
                    super::ref_resolution::resolve_at(origin, repository, reference, recipe)
                        .await
                        .ok()
                }
                Err(_) => None,
            }
        } else if metadata["visibility"] == "private" {
            match self
                .resolve_private_project_source(workspace, operation)
                .await
            {
                Ok(source) => Some(source),
                Err(error)
                    if error.status == axum::http::StatusCode::FORBIDDEN
                        || error.status == axum::http::StatusCode::UNAUTHORIZED =>
                {
                    return Err(error);
                }
                Err(_) => None,
            }
        } else {
            None
        };
        let pilot = if let Some(expected) = config["stagingProjectCachePilotSha256"].as_str() {
            let authority = super::pilot::current(chrono::Utc::now().timestamp_millis())?
                .ok_or_else(|| ApiError::forbidden("staging_project_cache_pilot_fenced"))?;
            if authority.pilot.fingerprint()? != expected
                || config["managedCandidateSha256"] != authority.pilot.candidate_sha256
            {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
            authority.pilot.source_allowed(
                owner,
                project,
                resolved
                    .as_ref()
                    .ok_or_else(|| ApiError::unavailable("project_cache_source_unavailable"))?,
            )?;
            Some(authority)
        } else {
            None
        };
        // Source IO can outlive the short membership cache. Recheck the
        // initiating actor before entering any SQL transaction/provider effect.
        self.allocation_scope_access(workspace, operation, chrono::Utc::now().timestamp_millis())
            .await?;
        // A failed anonymous proof is an optimization miss. Never expose cached
        // source to diagnostic ingress on a repository that became private.
        let mut tx = self.shared_pool().begin().await?;
        let current: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 FOR UPDATE",
        )
        .bind(&job.resource_id)
        .fetch_one(&mut *tx)
        .await?;
        let current_op:Value=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(operation["operation_id"].as_str()).bind(&job.resource_id).fetch_one(&mut *tx).await?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs j JOIN zuse_rust_control_owners o ON o.resource_id=j.resource_id AND o.resource_kind='workspace' AND o.backend='rust' AND o.generation=j.resource_generation WHERE j.job_id=$1 AND j.lease_token=$2 AND j.state='running' AND j.lease_expires_at>$3)").bind(&job.id).bind(&job.lease_token).bind(chrono::Utc::now().timestamp_millis()).fetch_one(&mut *tx).await?;
        if !active
            || current_op["phase"] != "planned"
            || current_op["machine_id"].is_string()
            || current_op["generation"] != job.generation
            || current["request_config"]["runtimeGeneration"] != job.generation
            || current["request_config"]["rustAllocationOperationId"] != current_op["operation_id"]
            || current["desired_state"] != "ready"
            || current["request_config"] != workspace["request_config"]
        {
            return Err(ApiError::conflict("project_cache_allocation_fenced"));
        }
        // A project disconnection/visibility change during HTTP must win.
        let same:Option<String>=sqlx::query_scalar("SELECT project_id FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true AND repository_identity=$3 AND visibility=$4 FOR SHARE").bind(owner).bind(project).bind(repository).bind(metadata["visibility"].as_str()).fetch_optional(&mut *tx).await?;
        if same.is_none() {
            return Err(ApiError::conflict("project_cache_project_changed"));
        }
        let old_cache = current["request_config"]["rustProjectCache"].clone();
        let base = old_cache["identity"]["baseArtifactId"]
            .as_str()
            .or_else(|| current_op["snapshot_id"].as_str())
            .ok_or_else(|| ApiError::conflict("project_cache_allocation_fenced"))?;
        let release = current["request_config"]["managedReleaseId"]
            .as_str()
            .unwrap_or_default();
        let mut next = current["request_config"].clone();
        let base_binding = if cached {
            old_cache["baseArtifactBinding"].clone()
        } else {
            next["providerArtifactBinding"].clone()
        };
        let mut hit = None;
        if let Some(source) = &resolved {
            if !next["rustResolvedSource"].is_null()
                && next["rustResolvedSource"]["commit"] != source.commit
            {
                return Err(ApiError::conflict("project_cache_source_identity_changed"));
            }
            next["rustResolvedSource"] = json!({"version":1,"operationId":current_op["operation_id"],"repositoryIdentity":repository,"requestedRef":current["base_ref"],"commit":source.commit});
            next["rustRepositoryCheckout"]["baseRef"] = json!(source.commit);
            next["rustRepositoryCheckout"]["sourceCommit"] = json!(source.commit);
            hit = if !super::enabled() { None } else { super::selection::for_resolved_task_tx(
                &mut tx,
                owner,
                project,
                repository,
                current["provider"].as_str().unwrap_or_default(),
                release,
                base,
                &source.commit,
                &json!(source.recipe),
                &source.recipe_source,
            )
            .await? };
            // A later task at a new commit gets its own source-pinned preparation
            // request while identical requests share the same immutable artifact.
            if let Some(revision) = next["environmentRevision"].as_i64().filter(|_| super::enabled()) {
                use sha2::Digest;
                let configured: zuse_protocol::cloud_computers::EnvironmentRecipe =
                    serde_json::from_value(next["environmentRecipe"].clone())
                        .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?;
                let configured_digest = format!(
                    "{:x}",
                    sha2::Sha256::digest(
                        serde_json::to_vec(&configured)
                            .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?
                    )
                );
                sqlx::query("INSERT INTO zuse_rust_project_preparation_requests(request_id,owner_id,actor_id,actor,project_id,environment_revision,recipe_digest,repository_identity,requested_ref,state,next_attempt_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'queued',$10,$10,$10) ON CONFLICT(owner_id,project_id,environment_revision,requested_ref) DO NOTHING")
                    .bind(format!("prep_{}",uuid::Uuid::new_v4().simple())).bind(owner).bind(next["rustActorId"].as_str()).bind(if owner.starts_with("organization:"){json!({"membershipId":next["sharingPolicy"]["creatorMembershipId"]})}else{Value::Null}).bind(project).bind(revision).bind(configured_digest).bind(repository).bind(&source.commit).bind(chrono::Utc::now().timestamp_millis()).execute(&mut *tx).await?;
            }
        }
        let artifact = if let Some(hit) = hit {
            next["rustProjectCache"] = hit.config();
            next["rustProjectCache"]["baseArtifactBinding"] = base_binding;
            next["repositoryCache"] = json!("project-environment");
            next["rustRepositoryCheckout"]["cachePath"] = json!(format!(
                "/opt/zuse/project-caches/{}/repository.git",
                hit.cache_key
            ));
            next["rustRepositoryCheckout"]["fetchRemote"] = json!(false);
            next["providerArtifactBinding"] = json!(hit.artifact_binding);
            hit.artifact_id
        } else {
            let active = if let Some(authority) = &pilot {
                let candidate = authority
                    .candidate_tx(&mut tx, chrono::Utc::now().timestamp_millis())
                    .await?;
                candidate.artifact_id == base
            } else {
                sqlx::query_scalar("SELECT coalesce((SELECT state='verified' AND artifact_id=$3 FROM zuse_rust_managed_base_admission_lock($1,$2)),false)").bind(current["provider"].as_str()).bind(release).bind(base).fetch_one(&mut *tx).await?
            };
            if !active {
                return Err(ApiError::conflict("project_cache_managed_base_retired"));
            }
            next.as_object_mut().unwrap().remove("rustProjectCache");
            next["providerArtifactBinding"] = base_binding;
            next["repositoryCache"] = json!("none");
            if let Some(checkout) = next["rustRepositoryCheckout"].as_object_mut() {
                checkout.remove("cachePath");
            }
            next["rustRepositoryCheckout"]["fetchRemote"] = json!(true);
            base.to_owned()
        };
        let changed = artifact != current_op["snapshot_id"]
            || next["providerArtifactBinding"]
                != current["request_config"]["providerArtifactBinding"];
        *workspace=sqlx::query_scalar("UPDATE api_cloud_workspaces w SET request_config=$2,revision=revision+1 WHERE workspace_id=$1 RETURNING to_jsonb(w)").bind(&job.resource_id).bind(next).fetch_one(&mut *tx).await?;
        *operation=sqlx::query_scalar("UPDATE zuse_rust_allocations a SET snapshot_id=$2 WHERE operation_id=$1 RETURNING to_jsonb(a)").bind(current_op["operation_id"].as_str()).bind(artifact).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(changed)
    }
}
