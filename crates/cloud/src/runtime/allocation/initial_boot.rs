//! Fresh clean-image startup is a distinct effect before any bootstrap credential.
use super::*;
use crate::managed_bases::bootstrap::{InitialRuntimeBootstrap, Purpose, Request};
fn denied() -> ApiError {
    ApiError::conflict("cloud_initial_runtime_bootstrap_fenced")
}
pub(in crate::runtime) fn request(
    row: &Value,
    operation: &Value,
    machine: &str,
    purpose: Purpose,
    disk: &str,
) -> Result<Option<Request>, ApiError> {
    let Some(spec) = InitialRuntimeBootstrap::from_config(&row["request_config"])? else {
        return Ok(None);
    };
    let provider = serde_json::from_value(row["provider"].clone()).map_err(|_| denied())?;
    if !spec.valid(provider)
        || row["request_config"]["rustProviderControllerVersion"] != 1
        || row["request_config"]["managedReleaseId"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err(denied());
    }
    let r = Request {
        version: 1,
        provider,
        purpose,
        operation_id: required(operation, "operation_id")?.into(),
        workspace_id: required(row, "workspace_id")?.into(),
        generation: operation["generation"].as_i64().ok_or_else(denied)?,
        storage_incarnation_id: disk.into(),
        source_machine_id: machine.into(),
        image_pins: spec.pins(),
    };
    if !r.valid() {
        return Err(denied());
    }
    Ok(Some(r))
}
fn saved(row: &Value, request: &Request, source: &str) -> bool {
    let value = &row["request_config"]["initialRuntimeBootstrapReceipt"];
    value["sourceArtifactId"] == source && request.saved(&value["proof"])
}
pub(super) fn positive_create_source(request: &Request, source: &str, name: &str) -> Value {
    json!({"version":1,"provider":request.provider,"operationId":request.operation_id,
        "workspaceId":request.workspace_id,"generation":request.generation,
        "sourceArtifactId":source,"machineName":name,"machineId":request.source_machine_id,
        "storageIncarnationId":request.storage_incarnation_id,"imagePins":request.image_pins})
}
fn verified_create_source(
    row: &Value,
    request: &Request,
    source: &str,
    name: &str,
) -> Option<crate::providers::contract::VerifiedCreateSource> {
    (request.provider == zuse_protocol::cloud_computers::ProviderId::Boat
        && request.valid()
        && row["request_config"]["rustPositiveCreateReceipt"]
            == positive_create_source(request, source, name))
    .then(|| {
        crate::providers::contract::VerifiedCreateSource::from_fenced_positive_create(
            request, source, name,
        )
    })
}
impl RuntimeControl {
    #[tracing::instrument(name="cloud_allocation_stage", skip_all, fields(workspace=%job.resource_id, operation="bootstrap_fence", has_proof=proof.is_some()))]
    async fn initial_boot_allocation_fence(
        &self,
        job: &jobs::Job,
        request: &Request,
        source: &str,
        name: &str,
        proof: Option<&Value>,
    ) -> Result<Value, ApiError> {
        let mut tx = self.pool.begin().await?;
        let now = clock_ms();
        let (row, generation, op, live, capacity_observation) = if let Some(snapshot) =
            self.allocation_snapshot(&mut tx, job).await?
        {
            (
                snapshot.workspace,
                snapshot.generation,
                snapshot.operation,
                snapshot.live,
                Some(snapshot.capacity),
            )
        } else {
            let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
            let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3)").bind(&job.id).bind(&job.lease_token).bind(now).fetch_one(&mut *tx).await?;
            let operation:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE").bind(&request.operation_id).bind(&job.resource_id).bind(job.generation).fetch_optional(&mut *tx).await?;
            (row, generation, operation.ok_or_else(denied)?, live, None)
        };
        if !live
            || op["operation_id"] != request.operation_id
            || generation != job.generation
            || generation != request.generation
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["provider"] != request.provider.as_str()
            || row["provider_sandbox_id"] != request.source_machine_id
            || row["request_config"]["rustAllocationOperationId"] != request.operation_id
            || op["machine_id"] != request.source_machine_id
            || op["snapshot_id"] != source
            || op["machine_name"] != name
            || op["storage_incarnation_id"] != request.storage_incarnation_id
            || op["boot_expires_at"].as_i64().unwrap_or(0) <= now
            || !matches!(op["phase"].as_str(), Some("allocated" | "launching"))
            || super::super::repair::leases_blocked(&row)
            || super::super::recreation::leases_blocked(&row)
        {
            return Err(denied());
        }
        if row["request_config"]["runtimeGeneration"] != request.generation
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
                != request.storage_incarnation_id
        {
            return Err(denied());
        }
        if super::preparation_hold::active(&row, &op) {
            return Err(ApiError::conflict(super::preparation_hold::ACTIVE));
        }
        if !saved(&row, request, source)
            && (row["runtime_credential_hash"].is_string()
                || !row["request_config"]["runtimeBootstrapReceipt"].is_null()
                || !row["request_config"]["rustStorageEnrollment"].is_null()
                || row["runtime_state"] == "online")
        {
            return Err(denied());
        }
        let current = crate::managed_bases::bootstrap::InitialRuntimeBootstrap::from_config(
            &row["request_config"],
        )?
        .ok_or_else(denied)?;
        if !current.valid(request.provider) || current.pins() != request.image_pins {
            return Err(denied());
        }
        self.allocation_snapshot_capacity(&mut tx, &row, capacity_observation.as_ref(), now)
            .await?;
        let mut result = row.clone();
        if proof.is_none() && op["phase"] != "launching" {
            // The same fresh fence authorizes and persists this transition.
            // Avoid an earlier transaction repeating all of these reads.
            sqlx::query("UPDATE zuse_rust_allocations SET phase='launching',updated_at=$2 WHERE operation_id=$1")
                .bind(&request.operation_id).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE api_cloud_workspaces SET status_code=CASE WHEN runtime_state='online' THEN status_code ELSE 'starting-runtime' END,revision=revision+1,updated_at=GREATEST(updated_at+1,$2) WHERE workspace_id=$1")
                .bind(&job.resource_id).bind(now).execute(&mut *tx).await?;
        }
        if let Some(proof) = proof {
            if !request.saved(proof) {
                return Err(denied());
            }
            let mut config = row["request_config"].clone();
            config["initialRuntimeBootstrapReceipt"] =
                json!({"sourceArtifactId":source,"proof":proof});
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(&job.resource_id).bind(&config).bind(now).execute(&mut *tx).await?;
            result["request_config"] = config;
        }
        tx.commit().await?;
        Ok(result)
    }
    pub(super) async fn initial_allocation_boot(
        &self,
        provider: &dyn AllocationProvider,
        job: &jobs::Job,
        row: &Value,
        operation: &Value,
        machine: &str,
    ) -> Result<bool, ApiError> {
        let Some(request) = request(
            row,
            operation,
            machine,
            Purpose::InitialAllocation,
            required(operation, "storage_incarnation_id")?,
        )?
        else {
            return Ok(true);
        };
        let source = required(operation, "snapshot_id")?;
        let name = required(operation, "machine_name")?;
        // This fence already reads and validates the current allocation,
        // generation, lease, lifecycle, source, storage and capacity. Separate
        // allocation_state passes before and after it repeat those same reads.
        // External scope checks precede the final SQL fence. The returned
        // authorization therefore rechecks lease, source and lifecycle AFTER
        // that unlocked work, immediately before a private guest handoff.
        // First attempt: the helper receives no boot credential and only
        // observes/starts the image's resident runtime, so run it alongside the
        // pre-helper scope check and fence instead of after them. Its proof is
        // used only after both succeed, and the final proof fence below still
        // rechecks scope, lease, source and lifecycle before any credential.
        // Retries (receipt already saved) keep the original serial order.
        let mut overlapped = None;
        // Overlap only when the just-read row is a fresh, unbound launch, so an
        // already-bound or held runtime never sees a helper call.
        let unbound = !row["runtime_credential_hash"].is_string()
            && row["request_config"]["runtimeBootstrapReceipt"].is_null()
            && row["request_config"]["rustStorageEnrollment"].is_null()
            && row["runtime_state"] != "online"
            && row["desired_state"] == "ready"
            && !blocked(row)
            && !super::preparation_hold::active(row, operation);
        let mut current = if saved(row, &request, source) || !unbound {
            self.allocation_scope_access(row, operation, clock_ms()).await?;
            self.initial_boot_allocation_fence(job, &request, source, name, None)
                .await?
        } else {
            let authority = verified_create_source(row, &request, source, name);
            let (scope, fence, proof) = tokio::join!(
                self.allocation_scope_access(row, operation, clock_ms()),
                self.initial_boot_allocation_fence(job, &request, source, name, None),
                provider.bootstrap_initial_runtime_with_source_receipt(
                    machine,
                    source,
                    name,
                    &request,
                    authority.as_ref(),
                ),
            );
            scope?;
            let current = fence?;
            overlapped = Some(proof.map_err(provider_error)?);
            current
        };
        if saved(&current, &request, source) {
            return Ok(true);
        }
        for attempt in 0..2 {
            if let Some(proof) = overlapped.take() {
                self.allocation_scope_access(&current, operation, clock_ms()).await?;
                current = self
                    .initial_boot_allocation_fence(job, &request, source, name, proof.as_ref())
                    .await?;
                if proof.is_some() {
                    return Ok(true);
                }
                if current["request_config"]["rustFastStartVersion"] != 1 {
                    return Ok(false);
                }
                tokio::time::sleep(std::time::Duration::from_millis(75)).await;
                continue;
            }
            // Only the exact original helper is retried; it receives no boot
            // credential and reconciles its own journal instead of starting twice.
            let authority = verified_create_source(&current, &request, source, name);
            let proof = provider
                .bootstrap_initial_runtime_with_source_receipt(
                    machine,
                    source,
                    name,
                    &request,
                    authority.as_ref(),
                )
                .await
                .map_err(provider_error)?;
            self.allocation_scope_access(&current, operation, clock_ms()).await?;
            current = self
                .initial_boot_allocation_fence(job, &request, source, name, proof.as_ref())
                .await?;
            if proof.is_some() {
                return Ok(true);
            }
            if attempt != 0 || current["request_config"]["rustFastStartVersion"] != 1 {
                return Ok(false);
            }
            // A resident starting now should not force a durable-job reschedule
            // and another complete allocation pass. Keep this lane bounded.
            tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        }
        Ok(false)
    }
}
#[cfg(test)]
mod tests;
