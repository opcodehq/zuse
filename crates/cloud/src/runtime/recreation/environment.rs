//! Reapply the selected recipe only on an explicitly replaced computer. The
//! provider command is a private idempotent Supervisor RPC, never a user prompt.
use super::super::restore::RestoreProvider;
use super::*;
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub(in crate::runtime) fn command(request: &Value) -> String {
    format!(
        "printf '%s\\n' {} | sudo -n -u zuse python3 /usr/local/lib/zuse/runtime-supervisor.py --data-dir /home/zuse/.zuse-data rpc",
        quote(&request.to_string())
    )
}
pub(super) fn receipt_matches(request: &Value, receipt: &Value) -> bool {
    receipt["version"] == 1
        && receipt["status"] == "prepared"
        && receipt["environmentReapply"] == true
        && [
            "operationId",
            "archiveOperationId",
            "workspaceId",
            "ownerId",
            "generation",
            "storageIncarnationId",
            "archiveSha256",
        ]
        .iter()
        .all(|key| receipt[*key] == request[*key])
}
impl RuntimeControl {
    pub(in crate::runtime) async fn recreation_prepare_environment(
        &self,
        provider: &dyn RestoreProvider,
        job: &jobs::Job,
        machine: &str,
        now: i64,
    ) -> Result<bool, ApiError> {
        let (mut tx, row, archive_op, archive, restore) = self.restore_locked(job).await?;
        let Some(operation) = row["request_config"]["rustRecreation"]["operationId"]
            .as_str()
            .map(str::to_owned)
        else {
            tx.commit().await?;
            return Ok(true);
        };
        if row["request_config"]["rustRecreation"]["phase"] == "ready" {
            tx.commit().await?;
            return Ok(true);
        }
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(&operation).bind(&job.resource_id).fetch_one(&mut *tx).await?;
        if !matches!(
            op["phase"].as_str(),
            Some("restoring" | "waiting-for-runtime")
        ) || op["restore_operation_id"] != restore["operation_id"]
            || op["restore_generation"] != restore["generation"]
            || restore["machine_id"] != machine
            || op["archive_operation_id"] != archive.id
            || op["verified_archive_sha256"] != archive_op["verified_archive_sha256"]
            || op["owner_id"] != row["account_id"]
        {
            return Err(fenced());
        }
        let request = json!({"version":1,"action":"prepare-recreation","operationId":operation,"archiveOperationId":archive.id,"workspaceId":job.resource_id,"ownerId":row["account_id"],"generation":restore["generation"],"storageIncarnationId":op["storage_incarnation_id"],"archiveSha256":op["verified_archive_sha256"]});
        let prior = &restore["provider_effects"]["recreationEnvironment"];
        if prior["state"] == "ready" {
            if prior["requestId"] != operation || !receipt_matches(&request, &prior["receipt"]) {
                return Err(fenced());
            }
            tx.commit().await?;
            return Ok(true);
        }
        if !prior.is_null() && prior["requestId"] != operation {
            return Err(fenced());
        }
        // An uncertain RPC is reconciled by delivering this SAME operation to
        // the helper's durable prepare/ready marker. Never mint another ID.
        sqlx::query("UPDATE zuse_rust_restores SET provider_effects=jsonb_set(provider_effects,'{recreationEnvironment}',$2),updated_at=GREATEST(updated_at+1,$3) WHERE operation_id=$1").bind(restore["operation_id"].as_str()).bind(json!({"requestId":operation,"state":"submitting"})).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        let receipt = provider
            .prepare_recreation(machine, &request)
            .await
            .map_err(|_| ApiError::unavailable("computer_recreation_environment_not_prepared"))?;
        if !receipt_matches(&request, &receipt) {
            return Err(fenced());
        }
        let (mut tx, current, _, _, restore) = self.restore_locked(job).await?;
        if current["request_config"]["rustRecreation"]["operationId"] != operation
            || restore["machine_id"] != machine
            || restore["provider_effects"]["recreationEnvironment"]["requestId"] != operation
        {
            return Err(fenced());
        }
        sqlx::query("UPDATE zuse_rust_restores SET provider_effects=jsonb_set(provider_effects,'{recreationEnvironment}',$2),updated_at=GREATEST(updated_at+1,$3) WHERE operation_id=$1").bind(restore["operation_id"].as_str()).bind(json!({"requestId":operation,"state":"ready","receipt":receipt})).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}
