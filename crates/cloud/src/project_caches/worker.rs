//! Bounded durable project preparation. External work never holds a SQL lock.
use super::*;
use async_trait::async_trait;
use sha2::Digest;
use std::sync::Arc;
use std::sync::OnceLock;
use zuse_protocol::cloud_computers::EnvironmentRecipe;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationPlan {
    pub identity: CacheIdentity,
    pub recipe: EnvironmentRecipe,
    pub base_artifact_binding: Option<crate::providers::artifact::ArtifactBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_runtime_bootstrap: Option<crate::managed_bases::bootstrap::InitialRuntimeBootstrap>,
    /// A bundle prepared outside the VM: no Git token/agent credential is sent.
    pub source_bundle_sha256: String,
}
impl PreparationPlan {
    pub fn validate(&self) -> Result<(), ApiError> {
        self.identity.validate()?;
        if self.initial_runtime_bootstrap.as_ref().is_some_and(|s| {
            !s.valid(self.identity.provider)
                || s.runtime_sha256 != self.identity.base_runtime_sha256
        }) {
            return Err(ApiError::conflict(
                "project_cache_initial_bootstrap_invalid",
            ));
        }
        self.recipe.validate().map_err(ApiError::bad_request)?;
        if !self.recipe.secret_bindings.is_empty() {
            return Err(ApiError::conflict("project_cache_runtime_secrets_required"));
        }
        let digest = format!(
            "{:x}",
            sha2::Sha256::digest(
                serde_json::to_vec(&self.recipe)
                    .map_err(|_| ApiError::bad_request("environment_recipe_invalid"))?
            )
        );
        if digest != self.identity.recipe_digest
            || self.source_bundle_sha256.len() != 64
            || !self
                .source_bundle_sha256
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(ApiError::bad_request("project_cache_plan_invalid"));
        }
        if let Some(binding) = &self.base_artifact_binding {
            binding
                .validate(self.identity.provider, &self.identity.base_artifact_id)
                .map_err(|_| ApiError::conflict("project_cache_artifact_binding_invalid"))?;
            if self.identity.base_artifact_version.as_deref()
                != Some(binding.artifact_version.as_str())
                || binding.runtime_sha256 != self.identity.base_runtime_sha256
            {
                return Err(ApiError::conflict("project_cache_artifact_binding_invalid"));
            }
        } else if self.identity.provider == zuse_protocol::cloud_computers::ProviderId::E2b {
            return Err(ApiError::conflict(
                "project_cache_artifact_binding_required",
            ));
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct Request {
    pub request_id: String,
    pub owner: String,
    pub actor_id: String,
    pub actor: Value,
    pub project: String,
    pub environment_revision: i64,
    pub requested_ref: String,
    pub repository_identity: String,
}
#[derive(Clone)]
pub struct Operation {
    pub operation_id: String,
    pub request: Request,
    pub plan: PreparationPlan,
    pub phase: String,
    pub resources: Value,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StepMode {
    Begin,
    Reconcile,
}
/// Reconcile must inspect the ORIGINAL provider operation/name; it must not submit
/// another allocation or capture. Original guest admission can reconnect only
/// with the identical plan under its durable local intent/DataLock. Cleanup can
/// retry deletion only after inspecting the exact original resource identity.
pub enum StepResult {
    Pending(Value),
    Advance {
        phase: String,
        resources: Value,
    },
    Ready {
        artifact_id: String,
        artifact_version: Option<String>,
        artifact_binding: Option<crate::providers::artifact::ArtifactBinding>,
        receipt: PreparationReceipt,
        resources: Value,
    },
    Failed {
        code: &'static str,
        resources: Value,
    },
}
/// Resource receipts contain only opaque operation/resource IDs and readiness
/// booleans. Provider stdout, credentials, request bodies and environment values
/// have no persistence channel through this interface.
fn validate_resources(value: &Value) -> Result<(), ApiError> {
    let Some(fields) = value.as_object() else {
        return Err(ApiError::unavailable(
            "project_cache_resource_receipt_invalid",
        ));
    };
    for (key, value) in fields {
        if !matches!(
            key.as_str(),
            "builderId"
                | "verifierId"
                | "artifactId"
                | "artifactVersion"
                | "operationId"
                | "requestName"
                | "builderAbsent"
                | "verifierAbsent"
                | "unpublishedArtifactAbsent"
                | "ready"
                | "captureNotSubmitted"
                | "builderNotSubmitted"
                | "verifierNotSubmitted"
        ) {
            return Err(ApiError::unavailable(
                "project_cache_resource_receipt_invalid",
            ));
        }
        if key.ends_with("Absent")
            || key == "ready"
            || matches!(
                key.as_str(),
                "captureNotSubmitted" | "builderNotSubmitted" | "verifierNotSubmitted"
            )
        {
            if !value.is_boolean() {
                return Err(ApiError::unavailable(
                    "project_cache_resource_receipt_invalid",
                ));
            }
        } else if !value.as_str().is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 256
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        }) {
            return Err(ApiError::unavailable(
                "project_cache_resource_receipt_invalid",
            ));
        }
    }
    Ok(())
}
#[async_trait]
pub trait Authorization: Send + Sync {
    /// Check current membership/enrollment and the original immutable author.
    async fn authorize(&self, request: &Request) -> Result<(), ApiError>;
}
#[async_trait]
pub trait PreparationBackend: Send + Sync {
    async fn reap_source(&self, _now: i64) -> Result<bool, ApiError> {
        Ok(false)
    }
    /// Resolve the exact commit/effective recipe, managed base and dependency pins.
    /// No task snapshot or agent connection may enter the returned plans.
    async fn resolve(&self, request: &Request) -> Result<Vec<PreparationPlan>, ApiError>;
    async fn step(&self, operation: &Operation, mode: StepMode) -> Result<StepResult, ApiError>;
}

struct Configuration {
    allow_new: bool,
    authorization: Arc<dyn Authorization>,
    backend: Arc<dyn PreparationBackend>,
}

pub struct CacheControl {
    pub(super) pool: PgPool,
    ready: OnceLock<bool>,
    configuration: OnceLock<Configuration>,
    cleanup_turn: std::sync::atomic::AtomicBool,
}
impl CacheControl {
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            ready: OnceLock::new(),
            configuration: OnceLock::new(),
            cleanup_turn: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn install(
        &self,
        authorization: Arc<dyn Authorization>,
        backend: Arc<dyn PreparationBackend>,
    ) -> Result<(), ApiError> {
        self.install_mode(authorization, backend, true)
    }
    pub(crate) fn install_mode(
        &self,
        authorization: Arc<dyn Authorization>,
        backend: Arc<dyn PreparationBackend>,
        allow_new: bool,
    ) -> Result<(), ApiError> {
        self.configuration
            .set(Configuration {
                authorization,
                backend,
                allow_new,
            })
            .map_err(|_| ApiError::conflict("project_preparation_already_configured"))
    }
    pub fn with_backend(
        self,
        authorization: Arc<dyn Authorization>,
        backend: Arc<dyn PreparationBackend>,
    ) -> Self {
        assert!(
            self.install(authorization, backend).is_ok(),
            "preparation already configured"
        );
        self
    }
    pub(crate) fn configured(&self) -> bool {
        self.configuration.get().is_some_and(|c| c.allow_new)
    }
    fn authorization(&self) -> Option<&Arc<dyn Authorization>> {
        self.configuration.get().map(|c| &c.authorization)
    }
    fn backend(&self) -> Option<&Arc<dyn PreparationBackend>> {
        self.configuration.get().map(|c| &c.backend)
    }
    pub async fn available(&self) -> Result<bool, ApiError> {
        if let Some(ready) = self.ready.get() {
            return Ok(*ready);
        }
        let ready = schema_ready(&self.pool).await?;
        let _ = self.ready.set(ready);
        Ok(ready)
    }
    pub(crate) async fn available_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
    ) -> Result<bool, ApiError> {
        if let Some(ready) = self.ready.get() {
            return Ok(*ready);
        }
        let ready = schema_ready_tx(tx).await?;
        let _ = self.ready.set(ready);
        Ok(ready)
    }
    fn request(row: &Value) -> Result<Request, ApiError> {
        let text = |field: &str| {
            row[field]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| ApiError::unavailable("project_preparation_record_invalid"))
        };
        Ok(Request {
            request_id: text("request_id")?,
            owner: text("owner_id")?,
            actor_id: text("actor_id")?,
            actor: row["actor"].clone(),
            project: text("project_id")?,
            environment_revision: row["environment_revision"]
                .as_i64()
                .ok_or_else(|| ApiError::unavailable("project_preparation_record_invalid"))?,
            requested_ref: text("requested_ref")?,
            repository_identity: text("repository_identity")?,
        })
    }
    async fn capacity(&self, owner: &str, now: i64) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        if let Some(authority) = super::pilot::current(now)? {
            if authority.pilot.owner_id != owner {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
            if !authority.pin.allow_staging_compute {
                crate::billing::require_capacity(&mut tx, owner, now).await?;
            }
        } else {
            crate::billing::require_capacity(&mut tx, owner, now).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    async fn block(&self, id: &str, lease: &str, code: &str, now: i64) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_rust_project_preparation_requests SET state='blocked',last_error_code=$3,next_attempt_at=$4,lease_id=NULL,lease_expires_at=NULL,updated_at=$5 WHERE request_id=$1 AND lease_id=$2").bind(id).bind(lease).bind(code).bind(now+30_000).bind(now).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn work_once(&self, now: i64) -> Result<bool, ApiError> {
        let started = std::time::Instant::now();
        if !self.available().await? {
            return Ok(false);
        }
        let suspended = if !self.configured() && self.backend().is_some() {
            sqlx::query("WITH waiting AS(SELECT request_id FROM zuse_rust_project_preparation_requests WHERE state IN('queued','blocked','resolving','preparing') AND coalesce(lease_expires_at,0)<=$1 AND last_error_code IS DISTINCT FROM 'project_preparation_disabled' ORDER BY request_id FOR UPDATE SKIP LOCKED LIMIT 32) UPDATE zuse_rust_project_preparation_requests r SET state='blocked',last_error_code='project_preparation_disabled',lease_id=NULL,lease_expires_at=NULL,updated_at=$1 FROM waiting w WHERE r.request_id=w.request_id").bind(now).execute(&self.pool).await?.rows_affected()>0
        } else {
            false
        };
        // Bound both resource and object cleanup fairly; an hourly source
        // tombstone sweep cannot starve VM reconciliation or vice versa.
        if self.retire_one(now).await? {
            return Ok(true);
        }
        let source_turn = self
            .cleanup_turn
            .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
        if source_turn {
            if let Some(backend) = self.backend() {
                if backend.reap_source(now).await? {
                    return Ok(true);
                }
            }
        }
        if self.work_operation(now).await? {
            return Ok(true);
        }
        if !source_turn {
            if let Some(backend) = self.backend() {
                if backend.reap_source(now).await? {
                    return Ok(true);
                }
            }
        }
        if !self.configured() {
            return Ok(suspended);
        }
        let lease = uuid::Uuid::new_v4().to_string();
        let row:Option<Value>=sqlx::query_scalar("WITH chosen AS (SELECT request_id FROM zuse_rust_project_preparation_requests WHERE state IN ('queued','blocked','resolving') AND next_attempt_at<=$1 AND (lease_id IS NULL OR lease_expires_at<=$1) ORDER BY next_attempt_at,request_id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE zuse_rust_project_preparation_requests r SET state=CASE WHEN state='queued' THEN 'resolving' ELSE state END,lease_id=$2,lease_expires_at=$1+60000,revision=revision+1,updated_at=$1 FROM chosen c WHERE r.request_id=c.request_id RETURNING to_jsonb(r)").bind(now).bind(&lease).fetch_optional(&self.pool).await?;
        let Some(row) = row else { return Ok(false) };
        let request = Self::request(&row)?;
        let Some(authority) = self.authorization() else {
            self.block(
                &request.request_id,
                &lease,
                "project_preparation_authority_unavailable",
                now,
            )
            .await?;
            return Ok(true);
        };
        if let Err(error) = authority.authorize(&request).await {
            self.block(&request.request_id, &lease, error.code, now)
                .await?;
            return Ok(true);
        }
        let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND status IN ('active','grace','ended') AND (status<>'ended' OR paid_through>$2) AND (paid_through IS NULL OR paid_through>$2))").bind(&request.owner).bind(now).fetch_one(&self.pool).await?;
        let pilot_allowance = super::pilot::current(now).ok().flatten().is_some_and(|a| {
            a.pilot.owner_id == request.owner
                && a.pilot.project_id == request.project
                && a.pin.allow_staging_compute
        });
        if !entitled && !pilot_allowance {
            self.block(
                &request.request_id,
                &lease,
                "cloud_entitlement_required",
                now,
            )
            .await?;
            return Ok(true);
        }
        if let Err(error) = self.capacity(&request.owner, now).await {
            self.block(&request.request_id, &lease, error.code, now)
                .await?;
            return Ok(true);
        }
        let Some(backend) = self.backend() else {
            self.block(
                &request.request_id,
                &lease,
                "project_cache_provider_preparation_unavailable",
                now,
            )
            .await?;
            return Ok(true);
        };
        // No database transaction is held across Git/provider/authority HTTP.
        let plans = match tokio::time::timeout(
            std::time::Duration::from_secs(40),
            backend.resolve(&request),
        )
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(error)) => {
                self.block(&request.request_id, &lease, error.code, now)
                    .await?;
                return Ok(true);
            }
            Err(_) => {
                self.block(
                    &request.request_id,
                    &lease,
                    "project_preparation_resolution_timeout",
                    now,
                )
                .await?;
                return Ok(true);
            }
        };
        if plans.is_empty() {
            self.block(
                &request.request_id,
                &lease,
                "managed_base_cache_runtime_required",
                now,
            )
            .await?;
            return Ok(true);
        }
        if let Err(error) = authority.authorize(&request).await {
            self.block(
                &request.request_id,
                &lease,
                error.code,
                now + i64::try_from(started.elapsed().as_millis()).unwrap_or(60_000),
            )
            .await?;
            return Ok(true);
        }
        let now = now + i64::try_from(started.elapsed().as_millis()).unwrap_or(60_000);
        let mut tx = self.pool.begin().await?;
        let current:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_project_preparation_requests r JOIN api_cloud_projects p ON p.project_id=r.project_id AND p.account_id=r.owner_id WHERE r.request_id=$1 AND r.lease_id=$2 AND r.lease_expires_at>$3 AND p.included=true AND r.environment_revision=(SELECT max(revision) FROM cloud_environment_versions WHERE owner_id=r.owner_id AND project_id=r.project_id))").bind(&request.request_id).bind(&lease).bind(now).fetch_one(&mut *tx).await?;
        if !current {
            tx.rollback().await?;
            return Ok(true);
        }
        let mut accepted = 0usize;
        for plan in plans {
            plan.validate()?;
            if plan.identity.owner_id != request.owner
                || plan.identity.project_id != request.project
                || plan.identity.repository_identity != request.repository_identity
            {
                return Err(ApiError::conflict("project_cache_scope_mismatch"));
            }
            if plan.identity.staging_pilot_sha256.is_some() {
                if !super::pilot::authorized_tx(&mut tx, &plan.identity, now)
                    .await
                    .unwrap_or(false)
                {
                    continue;
                }
            } else {
                let certification:Option<(String,String,Value)>=sqlx::query_as("SELECT c.architecture,c.runtime_sha256,c.receipt FROM cloud_project_cache_capabilities c JOIN cloud_managed_bases b USING(provider,release_id) WHERE c.provider=$1 AND c.release_id=$2 AND c.state='certified' AND b.state='verified' AND b.artifact_id=$3").bind(plan.identity.provider.as_str()).bind(&plan.identity.base_release_id).bind(&plan.identity.base_artifact_id).fetch_optional(&mut *tx).await?;
                if !certification.is_some_and(|(architecture, runtime, receipt)| {
                    architecture == plan.identity.architecture
                        && runtime == plan.identity.base_runtime_sha256
                        && serde_json::from_value::<CacheCapabilityReceipt>(receipt)
                            .is_ok_and(|r| r.valid(&runtime))
                }) {
                    continue;
                }
            }
            accepted += 1;
            let key = plan.identity.key()?;
            sqlx::query("INSERT INTO zuse_rust_project_cache_entries(cache_key,owner_id,project_id,provider,architecture,base_release_id,base_artifact_id,base_runtime_sha256,identity,recipe_digest,dependency_digest,source_commit,state,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'preparing',$13) ON CONFLICT(cache_key) DO NOTHING")
                .bind(&key).bind(&request.owner).bind(&request.project).bind(plan.identity.provider.as_str()).bind(&plan.identity.architecture).bind(&plan.identity.base_release_id).bind(&plan.identity.base_artifact_id).bind(&plan.identity.base_runtime_sha256).bind(json!(plan.identity)).bind(&plan.identity.recipe_digest).bind(plan.identity.dependency_digest()).bind(&plan.identity.source_commit).bind(now).execute(&mut *tx).await?;
            let stored: Value = sqlx::query_scalar(
                "SELECT identity FROM zuse_rust_project_cache_entries WHERE cache_key=$1",
            )
            .bind(&key)
            .fetch_one(&mut *tx)
            .await?;
            if stored != json!(plan.identity) {
                return Err(ApiError::conflict("project_cache_identity_conflict"));
            }
            sqlx::query("INSERT INTO zuse_rust_project_preparation_keys(request_id,cache_key) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(&request.request_id).bind(&key).execute(&mut *tx).await?;
            let already:bool=sqlx::query_scalar("SELECT state='ready' AND revoked_at IS NULL FROM zuse_rust_project_cache_entries WHERE cache_key=$1").bind(&key).fetch_one(&mut *tx).await?;
            if already {
                continue;
            }
            sqlx::query("INSERT INTO zuse_rust_project_cache_operations(operation_id,request_id,cache_key,owner_id,actor_id,actor,project_id,provider,state,phase,plan,next_attempt_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'queued','allocate',$9,$10,$10,$10) ON CONFLICT DO NOTHING")
                .bind(format!("cacheop_{}",uuid::Uuid::new_v4().simple())).bind(&request.request_id).bind(&key).bind(&request.owner).bind(&request.actor_id).bind(&request.actor).bind(&request.project).bind(plan.identity.provider.as_str()).bind(json!(plan)).bind(now).execute(&mut *tx).await?;
        }
        if accepted == 0 {
            sqlx::query("UPDATE zuse_rust_project_preparation_requests SET state='blocked',last_error_code='managed_base_cache_runtime_required',lease_id=NULL,lease_expires_at=NULL,next_attempt_at=$3,updated_at=$4 WHERE request_id=$1 AND lease_id=$2").bind(&request.request_id).bind(&lease).bind(now+30_000).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
        sqlx::query("UPDATE zuse_rust_project_preparation_requests SET state=CASE WHEN EXISTS(SELECT 1 FROM zuse_rust_project_preparation_keys k JOIN zuse_rust_project_cache_entries c USING(cache_key) WHERE k.request_id=$1 AND c.state<>'ready') THEN 'preparing' ELSE 'ready' END,last_error_code=NULL,lease_id=NULL,lease_expires_at=NULL,updated_at=$3 WHERE request_id=$1 AND lease_id=$2").bind(&request.request_id).bind(&lease).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
    async fn work_operation(&self, now: i64) -> Result<bool, ApiError> {
        let started = std::time::Instant::now();
        let lease = uuid::Uuid::new_v4().to_string();
        let row:Option<Value>=match sqlx::query_scalar("WITH chosen AS (SELECT q.operation_id FROM zuse_rust_project_cache_operations q WHERE q.state IN ('queued','running','uncertain','cleaning') AND q.next_attempt_at<=$1 AND (q.lease_id IS NULL OR q.lease_expires_at<=$1) AND NOT EXISTS(SELECT 1 FROM zuse_rust_project_cache_operations active WHERE active.owner_id=q.owner_id AND active.operation_id<>q.operation_id AND active.state IN ('running','uncertain','cleaning')) ORDER BY q.next_attempt_at,q.operation_id FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE zuse_rust_project_cache_operations o SET state=CASE WHEN state='queued' THEN 'running' ELSE state END,started_at=coalesce(started_at,$1),lease_id=$2,lease_expires_at=$1+120000,revision=revision+1,updated_at=$1 FROM chosen c WHERE o.operation_id=c.operation_id RETURNING to_jsonb(o)").bind(now).bind(&lease).fetch_optional(&self.pool).await {
            Ok(row)=>row,
            Err(error) if error.as_database_error().and_then(|e|e.constraint())==Some("zuse_rust_project_cache_one_active_owner")=>return Ok(false),
            Err(error)=>return Err(error.into()),
        };
        let Some(row) = row else { return Ok(false) };
        let operation_id = row["operation_id"]
            .as_str()
            .ok_or_else(|| ApiError::unavailable("project_cache_operation_invalid"))?;
        let plan: PreparationPlan = serde_json::from_value(row["plan"].clone())
            .map_err(|_| ApiError::unavailable("project_cache_operation_invalid"))?;
        plan.validate()?;
        let request_row: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(r) FROM zuse_rust_project_preparation_requests r WHERE request_id=$1",
        )
        .bind(
            row["request_id"]
                .as_str()
                .ok_or_else(|| ApiError::unavailable("project_cache_operation_invalid"))?,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(request_row) = request_row else {
            return Err(ApiError::unavailable("project_cache_request_missing"));
        };
        let request = Self::request(&request_row)?;
        let mut phase = row["phase"]
            .as_str()
            .ok_or_else(|| ApiError::unavailable("project_cache_operation_invalid"))?
            .to_owned();
        let Some(backend) = self.backend() else {
            sqlx::query("UPDATE zuse_rust_project_cache_operations SET lease_id=NULL,lease_expires_at=NULL,next_attempt_at=$3,last_error_code='project_cache_provider_preparation_unavailable' WHERE operation_id=$1 AND lease_id=$2").bind(operation_id).bind(&lease).bind(now+30_000).execute(&self.pool).await?;
            return Ok(true);
        };
        if !self.configured() {
            phase = "cleanup".into();
        }
        let current:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects p WHERE p.project_id=$1 AND p.account_id=$2 AND p.included=true) AND $3=(SELECT max(revision) FROM cloud_environment_versions WHERE owner_id=$2 AND project_id=$1)").bind(&request.project).bind(&request.owner).bind(request.environment_revision).fetch_one(&self.pool).await?;
        let authorized = if let Some(authority) = self.authorization() {
            authority.authorize(&request).await.is_ok()
        } else {
            false
        };
        if !current || !authorized {
            phase = "cleanup".into();
        }
        // Capability/base retirement denies new effects but must not strand a
        // previously created builder/artifact. Cleanup uses its original receipts.
        let certified = if plan.identity.staging_pilot_sha256.is_some() {
            let mut tx = self.pool.begin().await?;
            let allowed = super::pilot::authorized_tx(&mut tx, &plan.identity, now)
                .await
                .unwrap_or(false);
            tx.commit().await?;
            allowed
        } else {
            let certification:Option<(String,Value)>=sqlx::query_as("SELECT c.architecture,c.receipt FROM cloud_managed_bases b JOIN cloud_project_cache_capabilities c USING(provider,release_id) WHERE b.provider=$1 AND b.release_id=$2 AND b.state='verified' AND c.state='certified' AND b.artifact_id=$3 AND c.runtime_sha256=$4").bind(plan.identity.provider.as_str()).bind(&plan.identity.base_release_id).bind(&plan.identity.base_artifact_id).bind(&plan.identity.base_runtime_sha256).fetch_optional(&self.pool).await?;
            let certified = certification.is_some_and(|(architecture, receipt)| {
                architecture == plan.identity.architecture
                    && serde_json::from_value::<CacheCapabilityReceipt>(receipt)
                        .is_ok_and(|v| v.valid(&plan.identity.base_runtime_sha256))
            });
            certified
        };
        if !certified {
            phase = "cleanup".into();
        }
        if phase == "allocate" {
            let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(&request.owner).bind(now).fetch_one(&self.pool).await?;
            let pilot_allowance = super::pilot::current(now).ok().flatten().is_some_and(|a| {
                a.pilot.permits(&plan.identity).unwrap_or(false) && a.pin.allow_staging_compute
            });
            if !entitled && !pilot_allowance {
                // A lost create reply can already represent a billable builder.
                // Budget withdrawal must not prevent deletion of that original
                // resource. Cleanup never allocates or mints runtime authority.
                phase = "cleanup".into();
                sqlx::query("UPDATE zuse_rust_project_cache_operations SET last_error_code='cloud_entitlement_required' WHERE operation_id=$1 AND lease_id=$2").bind(operation_id).bind(&lease).execute(&self.pool).await?;
            }
        }
        if !matches!(phase.as_str(), "cleanup" | "publish") {
            let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND status IN ('active','grace','ended') AND (status<>'ended' OR paid_through>$2) AND (paid_through IS NULL OR paid_through>$2))").bind(&request.owner).bind(now).fetch_one(&self.pool).await?;
            let pilot_allowance = super::pilot::current(now).ok().flatten().is_some_and(|a| {
                a.pilot.permits(&plan.identity).unwrap_or(false) && a.pin.allow_staging_compute
            });
            let authorization = if entitled || pilot_allowance {
                self.capacity(&request.owner, now).await
            } else {
                Err(ApiError::forbidden("cloud_entitlement_required"))
            };
            if let Err(error) = authorization {
                phase = "cleanup".into();
                sqlx::query("UPDATE zuse_rust_project_cache_operations SET last_error_code=$3 WHERE operation_id=$1 AND lease_id=$2").bind(operation_id).bind(&lease).bind(error.code).execute(&self.pool).await?;
            }
        }
        // A dead preparation child must not retain a billable builder forever.
        // Deadline only switches to original-resource cleanup; it never forgets
        // uncertain IDs or manufactures an absence receipt.
        let checked_now = now + i64::try_from(started.elapsed().as_millis()).unwrap_or(60_000);
        let limit = i64::from(plan.recipe.setup_timeout_seconds) * 1000 + 600_000;
        if phase != "cleanup"
            && checked_now
                >= row["started_at"]
                    .as_i64()
                    .unwrap_or(now)
                    .saturating_add(limit)
        {
            phase = "cleanup".into();
            sqlx::query("UPDATE zuse_rust_project_cache_operations SET last_error_code='project_cache_preparation_deadline' WHERE operation_id=$1 AND lease_id=$2").bind(operation_id).bind(&lease).execute(&self.pool).await?;
        }
        let mut resources = row["resource_receipts"].clone();
        let effect_id = format!("{operation_id}:{phase}");
        let requested = resources["effects"][&phase]["requested"] == true;
        if !resources.is_object() {
            return Err(ApiError::unavailable("project_cache_operation_invalid"));
        }
        if !resources["effects"].is_object() {
            resources["effects"] = json!({});
        }
        resources["effects"][&phase] = json!({"requested":true,"operationId":effect_id});
        let effect_now = now + i64::try_from(started.elapsed().as_millis()).unwrap_or(60_000);
        let effect_statement = "UPDATE zuse_rust_project_cache_operations SET phase=$3,state=CASE WHEN $3='cleanup' THEN 'cleaning' ELSE 'running' END,resource_receipts=$4 WHERE operation_id=$1 AND lease_id=$2 AND lease_expires_at>$5 RETURNING true";
        let guarded = if plan.identity.staging_pilot_sha256.is_some()
            && matches!(phase.as_str(), "allocate" | "verify")
            && !requested
        {
            let mut tx = self.pool.begin().await?;
            if let Err(error) = super::pilot::capacity_tx(&mut tx, Some(operation_id)).await {
                // No provider effect was submitted. Keep this exact operation,
                // original name and plan eligible after the held task is gone.
                sqlx::query("UPDATE zuse_rust_project_cache_operations SET state='queued',next_attempt_at=$3,last_error_code=$4,lease_id=NULL,lease_expires_at=NULL WHERE operation_id=$1 AND lease_id=$2")
                    .bind(operation_id).bind(&lease).bind(effect_now+2_000).bind(error.code).execute(&mut *tx).await?;
                sqlx::query("UPDATE zuse_rust_project_preparation_requests SET state='blocked',last_error_code=$2,updated_at=$3 WHERE request_id=$1")
                    .bind(&request.request_id).bind(error.code).bind(effect_now).execute(&mut *tx).await?;
                tx.commit().await?;
                return Ok(true);
            }
            let guarded = sqlx::query_scalar(effect_statement)
                .bind(operation_id)
                .bind(&lease)
                .bind(&phase)
                .bind(&resources)
                .bind(effect_now)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(false);
            if guarded {
                sqlx::query("UPDATE zuse_rust_project_preparation_requests SET state='preparing',last_error_code=NULL,updated_at=$2 WHERE request_id=$1").bind(&request.request_id).bind(effect_now).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            guarded
        } else {
            sqlx::query_scalar(effect_statement)
                .bind(operation_id)
                .bind(&lease)
                .bind(&phase)
                .bind(&resources)
                .bind(effect_now)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or(false)
        };
        if !guarded {
            return Ok(true);
        }
        let operation = Operation {
            operation_id: operation_id.into(),
            request: request.clone(),
            plan: plan.clone(),
            phase: phase.clone(),
            resources: resources.clone(),
        };
        let mode = if requested {
            StepMode::Reconcile
        } else {
            StepMode::Begin
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(if plan.initial_runtime_bootstrap.is_some() {
                80
            } else {
                40
            }),
            backend.step(&operation, mode),
        )
        .await;
        let mut finished = now + i64::try_from(started.elapsed().as_millis()).unwrap_or(60_000);
        let outcome = match result {
            Ok(Ok(value)) => value,
            _ => {
                sqlx::query("UPDATE zuse_rust_project_cache_operations SET state='uncertain',last_error_code='project_cache_effect_outcome_uncertain',next_attempt_at=$3,lease_id=NULL,lease_expires_at=NULL,updated_at=$4 WHERE operation_id=$1 AND lease_id=$2").bind(operation_id).bind(&lease).bind(finished+5_000).bind(finished).execute(&self.pool).await?;
                return Ok(true);
            }
        };
        let still_authorized = if phase == "cleanup" {
            false
        } else if let Some(authority) = self.authorization() {
            authority.authorize(&request).await.is_ok()
        } else {
            false
        };
        let mut tx = self.pool.begin().await?;
        if phase == "publish" && matches!(&outcome, StepResult::Ready { .. }) {
            // Same registry→certificate→operation→entry order as admission and
            // request scheduling; no external IO is performed under these locks.
            let _: Vec<Value> = sqlx::query_scalar(
                "SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b",
            )
            .bind(plan.identity.provider.as_str())
            .bind(&plan.identity.base_release_id)
            .fetch_all(&mut *tx)
            .await?;
            let _: Vec<Value> = sqlx::query_scalar(
                "SELECT to_jsonb(c) FROM zuse_rust_project_cache_capability_lock($1,$2) c",
            )
            .bind(plan.identity.provider.as_str())
            .bind(&plan.identity.base_release_id)
            .fetch_all(&mut *tx)
            .await?;
        }
        finished = now + i64::try_from(started.elapsed().as_millis()).unwrap_or(60_000);
        let locked:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_project_cache_operations o WHERE operation_id=$1 AND lease_id=$2 AND lease_expires_at>$3 FOR UPDATE")
            .bind(operation_id).bind(&lease).bind(finished).fetch_optional(&mut *tx).await?;
        if locked.is_none() {
            tx.rollback().await?;
            return Ok(true);
        }
        let provider_receipt = match &outcome {
            StepResult::Pending(v) => v,
            StepResult::Advance { resources, .. }
            | StepResult::Ready { resources, .. }
            | StepResult::Failed { resources, .. } => resources,
        };
        validate_resources(provider_receipt)?;
        let (next, state, error, provider_resources) = match outcome {
            StepResult::Pending(value) => (
                phase.clone(),
                if phase == "cleanup" {
                    "cleaning"
                } else {
                    "running"
                },
                None,
                value,
            ),
            StepResult::Advance {
                phase: next,
                resources: value,
            } => {
                let expected = match phase.as_str() {
                    "allocate" => "prepare",
                    "prepare" => "quiesce",
                    "quiesce" => "capture",
                    "capture" => "retire-builder",
                    "retire-builder" => "verify",
                    "verify" => "retire-verifier",
                    "retire-verifier" => "publish",
                    "cleanup" => "complete",
                    _ => "",
                };
                if next != expected {
                    return Err(ApiError::unavailable("project_cache_phase_invalid"));
                }
                if phase == "cleanup" && next == "complete" {
                    if value["builderAbsent"] != true
                        || value["verifierAbsent"] != true
                        || value["unpublishedArtifactAbsent"] != true
                    {
                        return Err(ApiError::conflict("project_cache_cleanup_not_verified"));
                    }
                    (next, "deleted", None, value)
                } else if !still_authorized {
                    (
                        "cleanup".into(),
                        "cleaning",
                        Some("project_preparation_authorization_revoked"),
                        value,
                    )
                } else {
                    (next, "running", None, value)
                }
            }
            StepResult::Failed {
                code,
                resources: value,
            } => {
                // Failure never edits the previous compatible ready artifact.
                ("cleanup".into(), "cleaning", Some(code), value)
            }
            StepResult::Ready {
                artifact_id,
                artifact_version,
                artifact_binding,
                receipt,
                resources: value,
            } => {
                if !still_authorized || phase != "publish" {
                    (
                        "cleanup".into(),
                        "cleaning",
                        Some("project_preparation_authorization_revoked"),
                        value,
                    )
                } else {
                    if plan.identity.staging_pilot_sha256.is_some() {
                        if !super::pilot::authorized_tx(&mut tx, &plan.identity, finished)
                            .await
                            .unwrap_or(false)
                        {
                            return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
                        }
                    } else {
                        let current_base: Option<Value> = sqlx::query_scalar(
                        "SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b",
                    )
                    .bind(plan.identity.provider.as_str())
                    .bind(&plan.identity.base_release_id)
                    .fetch_optional(&mut *tx)
                    .await?;
                        if current_base.as_ref().is_none_or(|b| {
                            b["state"] != "verified"
                                || b["artifact_id"] != plan.identity.base_artifact_id
                        }) {
                            return Err(ApiError::conflict("project_cache_managed_base_retired"));
                        }
                        let capability:Option<(String,Value)>=sqlx::query_as("SELECT runtime_sha256,receipt FROM zuse_rust_project_cache_capability_lock($1,$2) WHERE state='certified'")
                        .bind(plan.identity.provider.as_str()).bind(&plan.identity.base_release_id).fetch_optional(&mut *tx).await?;
                        let valid_capability = capability.is_some_and(|(runtime, receipt)| {
                            runtime == plan.identity.base_runtime_sha256
                                && serde_json::from_value::<CacheCapabilityReceipt>(receipt)
                                    .is_ok_and(|v| v.valid(&runtime))
                        });
                        if !valid_capability {
                            return Err(ApiError::conflict(
                                "project_cache_runtime_capability_required",
                            ));
                        }
                    }
                    receipt.validate(&plan.identity, operation_id)?;
                    if receipt.source_bundle_sha256 != plan.source_bundle_sha256 {
                        return Err(ApiError::conflict("project_cache_source_bundle_mismatch"));
                    }
                    if value["builderAbsent"] != true
                        || value["verifierAbsent"] != true
                        || artifact_id.is_empty()
                        || artifact_id == plan.identity.base_artifact_id
                    {
                        return Err(ApiError::conflict("project_cache_resources_not_retired"));
                    }
                    if let Some(binding) = &artifact_binding {
                        binding
                            .validate(plan.identity.provider, &artifact_id)
                            .map_err(|_| {
                                ApiError::conflict("project_cache_artifact_binding_invalid")
                            })?;
                        if artifact_version.as_deref() != Some(binding.artifact_version.as_str())
                            || binding.runtime_sha256 != plan.identity.base_runtime_sha256
                        {
                            return Err(ApiError::conflict(
                                "project_cache_artifact_binding_invalid",
                            ));
                        }
                    }
                    if plan.identity.provider == zuse_protocol::cloud_computers::ProviderId::E2b
                        && artifact_binding.is_none()
                    {
                        return Err(ApiError::conflict(
                            "project_cache_artifact_binding_required",
                        ));
                    }
                    let key = plan.identity.key()?;
                    let existing:Value=sqlx::query_scalar("SELECT to_jsonb(c) FROM zuse_rust_project_cache_entries c WHERE cache_key=$1 FOR UPDATE").bind(&key).fetch_one(&mut *tx).await?;
                    if existing["state"] == "ready" {
                        if existing["artifact_id"] != artifact_id
                            || existing["receipt"] != json!(receipt)
                        {
                            return Err(ApiError::conflict(
                                "project_cache_ready_artifact_immutable",
                            ));
                        }
                    } else {
                        sqlx::query("UPDATE zuse_rust_project_cache_entries SET state='ready',artifact_id=$2,artifact_version=$3,artifact_binding=$4,receipt=$5,preparation_operation_id=$6,verified_at=$7 WHERE cache_key=$1 AND state='preparing'").bind(&key).bind(&artifact_id).bind(&artifact_version).bind(json!(artifact_binding)).bind(json!(receipt)).bind(operation_id).bind(finished).execute(&mut *tx).await?;
                    }
                    ("complete".into(), "ready", None, value)
                }
            }
        };
        let mut merged = resources;
        merged["provider"] = provider_resources;
        super::retirement::account_tx(
            &mut tx,
            locked.as_ref().unwrap(),
            &merged["provider"],
            finished,
        )
        .await?;
        sqlx::query("UPDATE zuse_rust_project_cache_operations SET phase=$3,state=$4,resource_receipts=$5,last_error_code=CASE WHEN $4='ready' THEN NULL ELSE coalesce($6,last_error_code) END,next_attempt_at=$7,lease_id=NULL,lease_expires_at=NULL,updated_at=$8 WHERE operation_id=$1 AND lease_id=$2")
            .bind(operation_id).bind(&lease).bind(&next).bind(state).bind(merged).bind(error).bind(finished+2_000).bind(finished).execute(&mut *tx).await?;
        if matches!(state, "ready" | "deleted") {
            sqlx::query("UPDATE zuse_rust_project_preparation_requests r SET state=CASE WHEN environment_revision<>(SELECT max(revision) FROM cloud_environment_versions WHERE owner_id=r.owner_id AND project_id=r.project_id) THEN 'superseded' WHEN EXISTS(SELECT 1 FROM zuse_rust_project_preparation_keys k JOIN zuse_rust_project_cache_entries e USING(cache_key) WHERE k.request_id=r.request_id AND e.state<>'ready') THEN CASE WHEN EXISTS(SELECT 1 FROM zuse_rust_project_preparation_keys k JOIN zuse_rust_project_cache_operations o USING(cache_key) WHERE k.request_id=r.request_id AND o.state IN ('queued','running','uncertain','cleaning')) THEN 'preparing' ELSE 'failed' END ELSE 'ready' END,last_error_code=(SELECT o.last_error_code FROM zuse_rust_project_preparation_keys k JOIN zuse_rust_project_cache_operations o USING(cache_key) WHERE k.request_id=r.request_id AND o.last_error_code IS NOT NULL ORDER BY o.updated_at DESC LIMIT 1),updated_at=$2 WHERE request_id IN(SELECT k.request_id FROM zuse_rust_project_preparation_keys k WHERE k.cache_key=(SELECT cache_key FROM zuse_rust_project_cache_operations WHERE operation_id=$1))").bind(operation_id).bind(finished).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}

impl crate::runtime::RuntimeControl {
    pub fn dispatch_project_preparation(self: &Arc<Self>, now: i64) {
        // Serial operator rehearsals retain source selection/authorization while
        // driving the same durable worker with awaited explicit ticks.
        if std::env::var("ZUSE_CLOUD_PROJECT_PREPARATION_BACKGROUND").as_deref() == Ok("0") {
            return;
        }
        use std::sync::atomic::Ordering;
        if self
            .project_cache_inflight
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
                    self.0
                        .project_cache_inflight
                        .store(false, Ordering::Release);
                }
            }
            let _reset = Reset(runtime.clone());
            match runtime.work_project_preparation_once(now).await {
                Ok(true) => crate::events::hint::send(),
                Err(error) => tracing::warn!(code = error.code, "Project preparation deferred"),
                _ => {}
            }
        });
    }
    pub async fn work_project_preparation_once(&self, now: i64) -> Result<bool, ApiError> {
        self.project_cache_control.work_once(now).await
    }
    /// Installed only with a provider-specific, credential-free preparation backend.
    pub fn with_project_preparation(
        mut self,
        authorization: Arc<dyn Authorization>,
        backend: Arc<dyn PreparationBackend>,
    ) -> Self {
        self.project_cache_control =
            CacheControl::from_pool(self.shared_pool()).with_backend(authorization, backend);
        self
    }
}
