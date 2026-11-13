//! Durable user image builds. Provider IO never runs under a SQL lock: each
//! phase records that its effect was requested, calls the backend, then
//! commits the receipt under the build's lease. A lost reply is reconciled by
//! inspecting the original resources on the next tick.
use super::*;
use crate::managed_bases::ManagedBaseRelease;
use crate::providers::boxd::Boxd;
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

/// Saving a recipe waits this long so a burst of edits builds once.
pub const SAVE_DEBOUNCE_MS: i64 = 15_000;
const LEASE_MS: i64 = 120_000;
const RETRY_MS: i64 = 300_000;
/// Provider work beyond the script's own limit: boots, capture, verify.
const OVERHEAD_MS: i64 = 20 * 60 * 1000;

pub struct UserImageControl {
    pool: PgPool,
    last_schedule: std::sync::atomic::AtomicI64,
    ready: OnceLock<bool>,
    backends: OnceLock<BTreeMap<ProviderId, Arc<dyn UserImageBackend>>>,
}

pub(crate) async fn schema_ready_tx(tx: &mut Transaction<'_, Postgres>) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='cloud_machine_images' AND column_name='image_id')").fetch_one(&mut **tx).await?)
}

/// Queue a build of the account's current recipe on every supported provider.
/// A waiting build is replaced, never duplicated: the newest recipe wins.
/// With no setup script, waiting builds are superseded and nothing is queued.
pub(crate) async fn enqueue_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    reason: &str,
    not_before: i64,
    now: i64,
) -> Result<bool, ApiError> {
    let latest: Option<(i64, Value)> = sqlx::query_as(
        "SELECT revision,recipe FROM cloud_machine_versions WHERE owner_id=$1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await?;
    let digest = latest.as_ref().and_then(|(_, recipe)| {
        serde_json::from_value::<MachineRecipe>(recipe.clone())
            .ok()
            .and_then(|recipe| image_digest(&recipe))
    });
    let (Some((revision, _)), Some(digest)) = (latest, digest) else {
        sqlx::query("UPDATE cloud_machine_images SET state='superseded',updated_at=$2 WHERE owner_id=$1 AND state='queued'")
            .bind(owner)
            .bind(now)
            .execute(&mut **tx)
            .await?;
        return Ok(false);
    };
    for provider in PROVIDERS {
        sqlx::query("INSERT INTO cloud_machine_images(image_id,owner_id,provider,reason,state,recipe_revision,recipe_digest,next_attempt_at,created_at,updated_at) VALUES($1,$2,$3,$4,'queued',$5,$6,$7,$8,$8) ON CONFLICT(owner_id,provider) WHERE state='queued' DO UPDATE SET recipe_revision=EXCLUDED.recipe_revision,recipe_digest=EXCLUDED.recipe_digest,reason=CASE WHEN cloud_machine_images.reason='manual' THEN 'manual' ELSE EXCLUDED.reason END,next_attempt_at=CASE WHEN EXCLUDED.reason='recipe' THEN EXCLUDED.next_attempt_at ELSE least(cloud_machine_images.next_attempt_at,EXCLUDED.next_attempt_at) END,error_code=NULL,updated_at=EXCLUDED.updated_at")
            .bind(format!("uimg_{}", uuid::Uuid::new_v4().simple()))
            .bind(owner)
            .bind(provider.as_str())
            .bind(reason)
            .bind(revision)
            .bind(&digest)
            .bind(not_before)
            .bind(now)
            .execute(&mut **tx)
            .await?;
    }
    Ok(true)
}

/// The newest verified base a builder may start from, with its fresh-boot pins.
pub(crate) fn base_from_row(row: &Value, provider: ProviderId) -> Option<ManagedBaseRelease> {
    if row["state"] != "verified" || row["runtime_manifest"].is_null() {
        return None;
    }
    let release: ManagedBaseRelease =
        serde_json::from_value(row["runtime_manifest"].clone()).ok()?;
    release.validate().ok()?;
    let bootstrap = release.initial_runtime_bootstrap.as_ref()?;
    if release.provider != provider
        || row["release_id"] != release.release_id.as_str()
        || row["artifact_id"] != release.artifact_id.as_str()
        || row["architecture"] != release.architecture.as_str()
        || !bootstrap.valid(provider)
        || release
            .manifest
            .command_admission
            .as_ref()
            .is_some_and(|c| c.runtime_sha256 != bootstrap.runtime_sha256)
    {
        return None;
    }
    Some(release)
}

impl UserImageControl {
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            last_schedule: std::sync::atomic::AtomicI64::new(0),
            ready: OnceLock::new(),
            backends: OnceLock::new(),
        }
    }
    pub fn install(
        &self,
        backends: BTreeMap<ProviderId, Arc<dyn UserImageBackend>>,
    ) -> Result<(), ApiError> {
        if backends.keys().any(|p| !PROVIDERS.contains(p)) {
            return Err(ApiError::conflict("user_image_provider_not_supported"));
        }
        self.backends
            .set(backends)
            .map_err(|_| ApiError::conflict("user_image_builder_already_configured"))
    }
    pub fn configured(&self) -> bool {
        self.backends.get().is_some_and(|b| !b.is_empty())
    }
    fn backend(&self, provider: &str) -> Option<&Arc<dyn UserImageBackend>> {
        let provider = super::provider_id(provider)?;
        self.backends.get()?.get(&provider)
    }
    pub async fn available(&self) -> Result<bool, ApiError> {
        if let Some(ready) = self.ready.get() {
            return Ok(*ready);
        }
        let mut tx = self.pool.begin().await?;
        let ready = schema_ready_tx(&mut tx).await?;
        tx.commit().await?;
        // Only a present schema is cached; a later migration is picked up.
        if ready {
            let _ = self.ready.set(true);
        }
        Ok(ready)
    }

    /// One bounded unit of work: advance a running build, else start one.
    pub async fn work_once(&self, now: i64) -> Result<bool, ApiError> {
        if !self.available().await? || !self.configured() {
            return Ok(false);
        }
        if self.step_one(now).await? || self.start_one(now).await? || self.retire_one(now).await? {
            return Ok(true);
        }
        use std::sync::atomic::Ordering;
        let last = self.last_schedule.load(Ordering::Acquire);
        if now - last < super::schedule::SCHEDULE_INTERVAL_MS
            || self
                .last_schedule
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Ok(false);
        }
        self.schedule_one(now).await
    }
    pub(crate) fn mark_scheduled(&self, now: i64) {
        self.last_schedule
            .fetch_max(now, std::sync::atomic::Ordering::AcqRel);
    }
    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn defer(
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        code: &str,
        at: i64,
        now: i64,
    ) -> Result<(), ApiError> {
        sqlx::query("UPDATE cloud_machine_images SET error_code=$2,next_attempt_at=$3,updated_at=$4 WHERE image_id=$1 AND state='queued'")
            .bind(id).bind(code).bind(at).bind(now).execute(&mut **tx).await?;
        Ok(())
    }

    /// Freeze a queued build against the current verified base and start it.
    async fn start_one(&self, now: i64) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let row: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(q) FROM cloud_machine_images q WHERE q.state='queued' AND q.next_attempt_at<=$1 AND NOT EXISTS(SELECT 1 FROM cloud_machine_images a WHERE a.owner_id=q.owner_id AND a.state IN('building','cleaning','retiring')) ORDER BY q.next_attempt_at,q.image_id FOR UPDATE SKIP LOCKED LIMIT 1")
            .bind(now).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(false);
        };
        let field = |name: &str| {
            row[name]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| ApiError::unavailable("user_image_record_invalid"))
        };
        let (id, owner, provider_name) =
            (field("image_id")?, field("owner_id")?, field("provider")?);
        let reason = field("reason")?;
        let provider = super::provider_id(&provider_name)
            .ok_or_else(|| ApiError::unavailable("user_image_record_invalid"))?;
        let latest: Option<(i64, Value)> = sqlx::query_as("SELECT revision,recipe FROM cloud_machine_versions WHERE owner_id=$1 ORDER BY revision DESC LIMIT 1")
            .bind(&owner).fetch_optional(&mut *tx).await?;
        let recipe = latest
            .as_ref()
            .and_then(|(_, r)| serde_json::from_value::<MachineRecipe>(r.clone()).ok());
        let (Some((revision, _)), Some(recipe)) = (latest, recipe) else {
            sqlx::query("UPDATE cloud_machine_images SET state='superseded',updated_at=$2 WHERE image_id=$1")
                .bind(&id).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        };
        let Some(digest) = image_digest(&recipe) else {
            sqlx::query("UPDATE cloud_machine_images SET state='superseded',updated_at=$2 WHERE image_id=$1")
                .bind(&id).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        };
        let Some(backend) = self.backend(&provider_name).cloned() else {
            Self::defer(
                &mut tx,
                &id,
                "user_image_provider_unavailable",
                now + RETRY_MS,
                now,
            )
            .await?;
            tx.commit().await?;
            return Ok(true);
        };
        if let Err(error) = crate::billing::require_capacity(&mut tx, &owner, now).await {
            Self::defer(&mut tx, &id, error.code, now + RETRY_MS, now).await?;
            tx.commit().await?;
            return Ok(true);
        }
        let bases: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(b) FROM cloud_managed_bases b WHERE provider=$1 AND state='verified' AND runtime_manifest IS NOT NULL ORDER BY created_at DESC,release_id DESC LIMIT 8")
            .bind(&provider_name).fetch_all(&mut *tx).await?;
        let Some(base) = bases.iter().find_map(|row| base_from_row(row, provider)) else {
            Self::defer(
                &mut tx,
                &id,
                "user_image_managed_base_unavailable",
                now + RETRY_MS,
                now,
            )
            .await?;
            tx.commit().await?;
            return Ok(true);
        };
        tx.commit().await?;
        // Provider read outside any lock.
        let version = match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            backend.base_version(provider, &base.artifact_id),
        )
        .await
        {
            Ok(Ok(Some(version))) => version,
            Ok(Ok(None)) => {
                let mut tx = self.pool.begin().await?;
                Self::defer(
                    &mut tx,
                    &id,
                    "user_image_managed_base_invalid",
                    now + RETRY_MS,
                    now,
                )
                .await?;
                tx.commit().await?;
                return Ok(true);
            }
            _ => {
                let mut tx = self.pool.begin().await?;
                Self::defer(
                    &mut tx,
                    &id,
                    "user_image_provider_unavailable",
                    now + 30_000,
                    now,
                )
                .await?;
                tx.commit().await?;
                return Ok(true);
            }
        };
        let bootstrap = base
            .initial_runtime_bootstrap
            .clone()
            .ok_or_else(|| ApiError::unavailable("user_image_managed_base_invalid"))?;
        let identity = UserImageIdentity {
            version: 1,
            owner_id: owner.clone(),
            provider,
            architecture: base.architecture.clone(),
            base_release_id: base.release_id.clone(),
            base_artifact_id: base.artifact_id.clone(),
            base_artifact_version: version,
            base_runtime_sha256: bootstrap.runtime_sha256.clone(),
            recipe_digest: digest.clone(),
        };
        let plan = BuildPlan {
            version: 1,
            image_id: id.clone(),
            identity,
            setup_script: recipe.setup_script.clone(),
            setup_timeout_seconds: recipe.setup_timeout_seconds,
            initial_runtime_bootstrap: bootstrap,
        };
        plan.validate()?;
        let key = plan.identity.key()?;
        let mut tx = self.pool.begin().await?;
        let still: Option<(String, i64)> = sqlx::query_as(
            "SELECT state,recipe_revision FROM cloud_machine_images WHERE image_id=$1 FOR UPDATE",
        )
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT max(revision) FROM cloud_machine_versions WHERE owner_id=$1",
        )
        .bind(&owner)
        .fetch_one(&mut *tx)
        .await?;
        if still.as_ref().is_none_or(|(state, _)| state != "queued") || current != Some(revision) {
            // Replaced or edited meanwhile; the next tick freezes the newest.
            tx.rollback().await?;
            return Ok(true);
        }
        let duplicate: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cloud_machine_images WHERE owner_id=$1 AND identity_key=$2 AND state IN('ready','building'))")
            .bind(&owner).bind(&key).fetch_one(&mut *tx).await?;
        if duplicate && !matches!(reason.as_str(), "manual" | "refresh") {
            sqlx::query("UPDATE cloud_machine_images SET state='superseded',identity_key=$2,updated_at=$3 WHERE image_id=$1")
                .bind(&id).bind(&key).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
        let started = sqlx::query("UPDATE cloud_machine_images SET state='building',phase='allocate',recipe_revision=$2,recipe_digest=$3,architecture=$4,base_release_id=$5,base_artifact_id=$6,base_artifact_version=$7,base_runtime_sha256=$8,identity_key=$9,plan=$10,resources='{}'::jsonb,error_code=NULL,log_excerpt=NULL,started_at=$11,next_attempt_at=$11,lease_id=NULL,lease_expires_at=NULL,revision=revision+1,updated_at=$11 WHERE image_id=$1 AND state='queued'")
            .bind(&id).bind(revision).bind(&digest).bind(&plan.identity.architecture).bind(&plan.identity.base_release_id)
            .bind(&plan.identity.base_artifact_id).bind(&plan.identity.base_artifact_version).bind(&plan.identity.base_runtime_sha256)
            .bind(&key).bind(json!(plan)).bind(now).execute(&mut *tx).await;
        match started {
            Ok(_) => {}
            Err(error)
                if error.as_database_error().and_then(|e| e.constraint())
                    == Some("cloud_machine_images_one_active") =>
            {
                tx.rollback().await?;
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn step_one(&self, now: i64) -> Result<bool, ApiError> {
        let started = std::time::Instant::now();
        let elapsed =
            |base: i64| base + i64::try_from(started.elapsed().as_millis()).unwrap_or(60_000);
        let lease = uuid::Uuid::new_v4().to_string();
        let row: Option<Value> = sqlx::query_scalar("WITH chosen AS(SELECT image_id FROM cloud_machine_images WHERE state IN('building','cleaning','retiring') AND next_attempt_at<=$1 AND (lease_id IS NULL OR lease_expires_at<=$1) ORDER BY next_attempt_at,image_id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE cloud_machine_images i SET lease_id=$2,lease_expires_at=$1+$3,revision=revision+1 FROM chosen c WHERE i.image_id=c.image_id RETURNING to_jsonb(i)")
            .bind(now).bind(&lease).bind(LEASE_MS).fetch_optional(&self.pool).await?;
        let Some(row) = row else { return Ok(false) };
        let invalid = || ApiError::unavailable("user_image_record_invalid");
        let id = row["image_id"].as_str().ok_or_else(invalid)?.to_owned();
        let state = row["state"].as_str().ok_or_else(invalid)?.to_owned();
        let plan: BuildPlan = serde_json::from_value(row["plan"].clone()).map_err(|_| invalid())?;
        plan.validate()?;
        if plan.image_id != id {
            return Err(invalid());
        }
        let release = |code: &'static str, at: i64| {
            let pool = self.pool.clone();
            let id = id.clone();
            let lease = lease.clone();
            async move {
                sqlx::query("UPDATE cloud_machine_images SET lease_id=NULL,lease_expires_at=NULL,next_attempt_at=$3,error_code=CASE WHEN state='building' THEN $4 ELSE error_code END,updated_at=$5 WHERE image_id=$1 AND lease_id=$2")
                    .bind(&id).bind(&lease).bind(at).bind(code).bind(now).execute(&pool).await?;
                Ok::<_, ApiError>(true)
            }
        };
        let Some(backend) = self.backend(plan.identity.provider.as_str()).cloned() else {
            return release("user_image_provider_unavailable", now + RETRY_MS).await;
        };
        let mut phase = row["phase"].as_str().ok_or_else(invalid)?.to_owned();
        let mut failure: Option<&'static str> = None;
        if state == "building" && phase != "publish" {
            let deadline = row["started_at"]
                .as_i64()
                .unwrap_or(now)
                .saturating_add(i64::from(plan.setup_timeout_seconds) * 1000 + OVERHEAD_MS);
            if now >= deadline {
                failure = Some("user_image_build_deadline");
            } else {
                // A build allocates billable machines; budget withdrawal stops it
                // and cleanup still deletes whatever it already created.
                let mut tx = self.pool.begin().await?;
                let capacity = crate::billing::require_capacity(&mut tx, &plan.owner(), now).await;
                tx.commit().await?;
                if let Err(error) = capacity {
                    failure = Some(error.code);
                }
            }
        }
        if let Some(code) = failure {
            phase = "cleanup".into();
            sqlx::query("UPDATE cloud_machine_images SET state='cleaning',phase='cleanup',error_code=$3 WHERE image_id=$1 AND lease_id=$2")
                .bind(&id).bind(&lease).bind(code).execute(&self.pool).await?;
        }
        let mut resources = row["resources"].clone();
        if !resources.is_object() {
            return Err(invalid());
        }
        if !resources["effects"].is_object() {
            resources["effects"] = json!({});
        }
        let requested = resources["effects"][&phase]["requested"] == true;
        resources["effects"][&phase] = json!({"requested":true});
        let guarded: bool = sqlx::query_scalar("UPDATE cloud_machine_images SET resources=$3 WHERE image_id=$1 AND lease_id=$2 AND lease_expires_at>$4 RETURNING true")
            .bind(&id).bind(&lease).bind(&resources).bind(elapsed(now)).fetch_optional(&self.pool).await?.unwrap_or(false);
        if !guarded {
            return Ok(true);
        }
        let build = Build {
            image_id: id.clone(),
            owner_id: plan.owner(),
            plan: plan.clone(),
            phase: phase.clone(),
            resources: resources.clone(),
            artifact_id: row["artifact_id"].as_str().map(str::to_owned),
        };
        let mode = if requested {
            StepMode::Reconcile
        } else {
            StepMode::Begin
        };
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(90),
            backend.step(&build, mode),
        )
        .await;
        let finished = elapsed(now);
        let outcome = match outcome {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                tracing::warn!(code = error.code, image = %id, phase = %phase, "User image step deferred");
                return release("user_image_effect_outcome_uncertain", finished + 5_000).await;
            }
            Err(_) => {
                return release("user_image_effect_outcome_uncertain", finished + 5_000).await;
            }
        };
        let receipt = match &outcome {
            Step::Pending(v) => v,
            Step::Advance { resources, .. }
            | Step::Ready { resources, .. }
            | Step::Failed { resources, .. } => resources,
        };
        validate_resources(receipt)?;
        let mut tx = self.pool.begin().await?;
        if phase == "publish" && matches!(outcome, Step::Ready { .. }) {
            // Same registry lock admission takes: a base retired meanwhile
            // cannot gain a new image.
            let base: Option<Value> = sqlx::query_scalar(
                "SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b",
            )
            .bind(plan.identity.provider.as_str())
            .bind(&plan.identity.base_release_id)
            .fetch_optional(&mut *tx)
            .await?;
            if base.as_ref().is_none_or(|b| {
                b["state"] != "verified"
                    || b["artifact_id"] != plan.identity.base_artifact_id.as_str()
            }) {
                sqlx::query("UPDATE cloud_machine_images SET state='cleaning',phase='cleanup',error_code='user_image_managed_base_retired',lease_id=NULL,lease_expires_at=NULL,next_attempt_at=$3,updated_at=$3 WHERE image_id=$1 AND lease_id=$2")
                    .bind(&id).bind(&lease).bind(finished).execute(&mut *tx).await?;
                tx.commit().await?;
                return Ok(true);
            }
        }
        let locked: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(i) FROM cloud_machine_images i WHERE image_id=$1 AND lease_id=$2 AND lease_expires_at>$3 FOR UPDATE")
            .bind(&id).bind(&lease).bind(finished).fetch_optional(&mut *tx).await?;
        let Some(locked) = locked else {
            tx.rollback().await?;
            return Ok(true);
        };
        let current_state = locked["state"].as_str().unwrap_or_default().to_owned();
        let mut merged = resources;
        let mut set_artifact: Option<(String, Option<String>)> = None;
        let mut log: Option<String> = None;
        let (next_phase, next_state, error, wait) = match outcome {
            Step::Pending(value) => {
                merged["provider"] = value;
                (
                    phase.clone(),
                    current_state.clone(),
                    None,
                    if phase == "setup" { 10_000 } else { 2_000 },
                )
            }
            Step::Advance {
                phase: next,
                resources: value,
            } => {
                if super::next_phase(&phase) != Some(next.as_str()) {
                    return Err(ApiError::unavailable("user_image_phase_invalid"));
                }
                match phase.as_str() {
                    "cleanup" => {
                        if value["builderAbsent"] != true
                            || value["verifierAbsent"] != true
                            || value["unpublishedArtifactAbsent"] != true
                        {
                            return Err(ApiError::conflict("user_image_cleanup_not_verified"));
                        }
                    }
                    "retire" if value["artifactAbsent"] != true => {
                        return Err(ApiError::conflict("user_image_retirement_not_verified"));
                    }
                    _ => {}
                }
                merged["provider"] = value;
                let state = match next.as_str() {
                    "complete" => "failed".to_owned(),
                    "retired" => "retired".to_owned(),
                    _ => current_state.clone(),
                };
                (next, state, None, 0)
            }
            Step::Failed {
                code,
                log: output,
                resources: value,
            } => {
                merged["provider"] = value;
                log = output.map(|raw| log_excerpt(&raw));
                if current_state == "building" {
                    ("cleanup".to_owned(), "cleaning".to_owned(), Some(code), 0)
                } else {
                    // Cleanup and retirement only retry; they never fail over.
                    (phase.clone(), current_state.clone(), None, 30_000)
                }
            }
            Step::Ready {
                artifact_id,
                artifact_version,
                resources: value,
            } => {
                if phase != "publish"
                    || current_state != "building"
                    || value["builderAbsent"] != true
                    || value["verifierAbsent"] != true
                    || artifact_id.is_empty()
                    || artifact_id == plan.identity.base_artifact_id
                    || value["artifactId"] != artifact_id.as_str()
                {
                    return Err(ApiError::conflict("user_image_publication_invalid"));
                }
                merged["provider"] = value;
                set_artifact = Some((artifact_id, artifact_version));
                ("complete".to_owned(), "ready".to_owned(), None, 0)
            }
        };
        if let Some((artifact, version)) = set_artifact {
            sqlx::query("UPDATE cloud_machine_images SET artifact_id=$2,artifact_version=$3,built_at=$4 WHERE image_id=$1")
                .bind(&id).bind(artifact).bind(version).bind(finished).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE cloud_machine_images SET phase=$3,state=$4,resources=$5,error_code=CASE WHEN $4='ready' THEN NULL ELSE coalesce($6,error_code) END,log_excerpt=coalesce($7,log_excerpt),retired_at=CASE WHEN $4='retired' THEN $8 ELSE retired_at END,next_attempt_at=$9,lease_id=NULL,lease_expires_at=NULL,updated_at=$8 WHERE image_id=$1 AND lease_id=$2")
            .bind(&id).bind(&lease).bind(&next_phase).bind(&next_state).bind(&merged).bind(error).bind(log)
            .bind(finished).bind(finished + wait).execute(&mut *tx).await?;
        tx.commit().await?;
        if next_state == "ready" {
            tracing::info!(image = %id, "User image published");
        }
        Ok(true)
    }
}

impl BuildPlan {
    pub fn owner(&self) -> String {
        self.identity.owner_id.clone()
    }
}

impl crate::runtime::RuntimeControl {
    /// Background tick beside project preparation: never blocks lifecycle work.
    pub fn dispatch_user_images(self: &Arc<Self>, now: i64) {
        use std::sync::atomic::Ordering;
        if !self.user_image_control.configured()
            || self
                .user_image_inflight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let runtime = self.clone();
        tokio::spawn(async move {
            struct Reset(Arc<crate::runtime::RuntimeControl>);
            impl Drop for Reset {
                fn drop(&mut self) {
                    self.0.user_image_inflight.store(false, Ordering::Release);
                }
            }
            let _reset = Reset(runtime.clone());
            match runtime.user_image_control.work_once(now).await {
                Ok(true) => crate::events::hint::send(),
                Err(error) => tracing::warn!(code = error.code, "User image build deferred"),
                _ => {}
            }
        });
    }
    /// Builders are on wherever the worker has Boxd credentials, unless
    /// `ZUSE_CLOUD_USER_IMAGES=0`. Without them accounts use the managed base.
    pub async fn configure_user_images_from_env(&self) -> Result<bool, ApiError> {
        if std::env::var("ZUSE_CLOUD_USER_IMAGES").as_deref() == Ok("0")
            || !self.user_image_control.available().await?
        {
            return Ok(false);
        }
        let Ok(key) = std::env::var("BOXD_API_KEY") else {
            return Ok(false);
        };
        let mut provider = Boxd::new(key, std::env::var("BOXD_ORG").ok())
            .map_err(|_| ApiError::unavailable("user_image_provider_invalid"))?;
        if let Ok(base) = std::env::var("BOXD_BASE_URL") {
            provider = provider
                .with_base_url(&base)
                .map_err(|_| ApiError::unavailable("user_image_provider_invalid"))?;
        }
        let mut backends: BTreeMap<ProviderId, Arc<dyn UserImageBackend>> = BTreeMap::new();
        backends.insert(
            ProviderId::Boxd,
            Arc::new(super::boxd::BoxdUserImages::new(Arc::new(provider))),
        );
        self.configure_user_images(backends)?;
        Ok(true)
    }
    pub async fn work_user_images_once(&self, now: i64) -> Result<bool, ApiError> {
        self.user_image_control.work_once(now).await
    }
    /// Install the provider builders this deployment has credentials for.
    pub fn configure_user_images(
        &self,
        backends: BTreeMap<ProviderId, Arc<dyn UserImageBackend>>,
    ) -> Result<(), ApiError> {
        self.user_image_control.install(backends)
    }
}
