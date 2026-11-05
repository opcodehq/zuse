//! Run under the guest's exclusive DataLock, BEFORE opening an EngineCore.
//! Admission/processed marks are not terminal outcome proofs. Missing ACKs are
//! quarantined atomically so background doc drains cannot race cold recovery.
use super::*;
pub fn quarantine_missing_ack(
    path: &Path,
    bootstrap: &CloudRuntimeBootstrap,
    key: &CommandKey,
) -> Result<usize> {
    if !path.is_file() {
        return Ok(0);
    }
    let mut db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "synchronous", "FULL")?;
    let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='zuse_cloud_command_receipts')",[],|row|row.get(0))?;
    if !exists {
        return Ok(0);
    }
    let storage = bootstrap
        .runtime_storage
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("runtime-storage-missing"))?;
    let identity:(String,String,String)=db.query_row("SELECT workspace_id,account_id,storage_incarnation_id FROM zuse_cloud_storage_identity WHERE singleton=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
    ensure!(
        identity
            == (
                bootstrap.workspace_id.clone(),
                bootstrap.zuse_account_id.clone(),
                storage.storage_incarnation_id.clone()
            ),
        "runtime-recovery-storage-fenced"
    );
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM zuse_cloud_command_receipts WHERE acknowledgment IS NULL",
        [],
        |row| row.get(0),
    )?;
    ensure!(count <= 256, "runtime-recovery-receipt-limit");
    let oversized:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM zuse_cloud_command_receipts WHERE acknowledgment IS NULL AND (length(envelope)>524288 OR length(lease_json)>524288))",[],|row|row.get(0))?;
    ensure!(!oversized, "runtime-recovery-receipt-invalid");
    let pending = {
        let mut statement=tx.prepare("SELECT command_id,workspace_id,session_id,storage_incarnation_id,fingerprint,envelope,lease_json FROM zuse_cloud_command_receipts WHERE acknowledgment IS NULL")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, workspace, session, incarnation, fingerprint, envelope, lease) in &pending {
        let envelope: CommandEnvelope = serde_json::from_str(envelope)?;
        let lease: RuntimeLease = serde_json::from_str(lease)?;
        ensure!(
            workspace == &bootstrap.workspace_id
                && incarnation == &storage.storage_incarnation_id
                && &envelope.workspace_id == workspace
                && &envelope.command_id == id
                && &envelope.session_id == session
                && &envelope.fingerprint == fingerprint
                && lease.storage_incarnation_id == *incarnation
                && lease.command.command_id == *id
                && lease.runtime_generation > 0
                && lease.runtime_generation <= bootstrap.runtime_generation
                && serde_json::to_value(&lease.command)? == serde_json::to_value(&envelope)?,
            "runtime-recovery-receipt-fenced"
        );
        // Verify the original encrypted command identity; never execute it or
        // infer success from a canonical pending entry/processed_commands mark.
        let _plaintext = key.decrypt(&envelope)?;
        let ack = RuntimeAcknowledgment {
            command_id: id.clone(),
            lease_token: lease.lease_token.clone(),
            fingerprint: fingerprint.clone(),
            state: AcknowledgmentState::OutcomeUnknown,
            category: Some("runtime-process-lost-before-terminal-ack".into()),
            result_iv: None,
            result_ciphertext: None,
        };
        tx.execute(
            "INSERT OR IGNORE INTO processed_commands(command_id,processed_at) VALUES(?1,?2)",
            params![id, now_ms()],
        )?;
        let changed=tx.execute("UPDATE zuse_cloud_command_receipts SET acknowledgment=?2,delivered=0 WHERE command_id=?1 AND acknowledgment IS NULL AND fingerprint=?3",params![id,serde_json::to_string(&ack)?,fingerprint])?;
        ensure!(changed == 1, "runtime-recovery-receipt-fenced");
    }
    tx.commit()?;
    Ok(pending.len())
}
