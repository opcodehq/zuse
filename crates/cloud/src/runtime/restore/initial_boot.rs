//! Only a new restore target is bootstrapped; restored/bound data is never repaired here.
use super::*;
use crate::managed_bases::bootstrap::{InitialRuntimeBootstrap, Purpose, Request};
fn matches(
    row: &Value,
    restore: &Value,
    op: &Value,
    request: &Request,
    source: &str,
    name: &str,
) -> Result<(), ApiError> {
    let spec = InitialRuntimeBootstrap::from_config(&row["request_config"])?.ok_or_else(fenced)?;
    if !spec.valid(request.provider)
        || spec.pins() != request.image_pins
        || restore["operation_id"] != request.operation_id
        || restore["generation"] != request.generation
        || restore["snapshot_id"] != source
        || restore["machine_name"] != name
        || restore["machine_id"] != request.source_machine_id
        || row["provider_sandbox_id"] != request.source_machine_id
        || row["request_config"]["runtimeGeneration"] != request.generation
        || op["storage_incarnation_id"] != request.storage_incarnation_id
        || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
            != request.storage_incarnation_id
    {
        return Err(fenced());
    }
    Ok(())
}
fn saved(row: &Value, request: &Request, source: &str) -> bool {
    let v = &row["request_config"]["initialRuntimeBootstrapReceipt"];
    v["sourceArtifactId"] == source && request.saved(&v["proof"])
}
impl RuntimeControl {
    pub(super) async fn initial_restore_boot(
        &self,
        provider: &dyn RestoreProvider,
        job: &jobs::Job,
    ) -> Result<bool, ApiError> {
        self.worker_job_scope_access(job, "restore", chrono::Utc::now().timestamp_millis())
            .await?;
        let (tx, row, op, _, restore) = self.restore_locked(job).await?;
        let source = required(&restore, "snapshot_id")?.to_owned();
        let name = required(&restore, "machine_name")?.to_owned();
        let machine = required(&restore, "machine_id")?.to_owned();
        let Some(request) = super::super::allocation::initial_boot::request(
            &row,
            &restore,
            &machine,
            Purpose::InitialRestore,
            required(&op, "storage_incarnation_id")?,
        )?
        else {
            tx.commit().await?;
            return Ok(true);
        };
        matches(&row, &restore, &op, &request, &source, &name)?;
        if saved(&row, &request, &source) {
            tx.commit().await?;
            return Ok(true);
        }
        if restore["phase"] != "allocated"
            || row["runtime_credential_hash"].is_string()
            || !row["request_config"]["runtimeBootstrapReceipt"].is_null()
            || !row["request_config"]["rustStorageEnrollment"].is_null()
        {
            return Err(fenced());
        }
        tx.commit().await?;
        let proof = provider
            .bootstrap_initial_runtime(&machine, &source, &name, &request)
            .await
            .map_err(|_| failed())?;
        self.worker_job_scope_access(job, "restore", chrono::Utc::now().timestamp_millis())
            .await?;
        let (mut tx, row, op, _, restore) = self.restore_locked(job).await?;
        matches(&row, &restore, &op, &request, &source, &name)?;
        if restore["phase"] != "allocated"
            || row["runtime_credential_hash"].is_string()
            || !row["request_config"]["runtimeBootstrapReceipt"].is_null()
        {
            return Err(fenced());
        }
        if let Some(proof) = &proof {
            if !request.saved(proof) {
                return Err(fenced());
            }
            let mut config = row["request_config"].clone();
            config["initialRuntimeBootstrapReceipt"] =
                json!({"sourceArtifactId":source,"proof":proof});
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(&job.resource_id).bind(config).bind(chrono::Utc::now().timestamp_millis()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(proof.is_some())
    }
}

#[cfg(test)]
mod tests;
