//! Distinguish an interrupted preparation from an unsupported legacy import.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{io::Write, path::Path};
use zuse_protocol::cloud::CloudRuntimeBootstrap;
const FILE: &str = "rust-preparation-ingress.json";
fn binding(boot: &CloudRuntimeBootstrap, enrollment: &str) -> Value {
    json!({"version":1,"workspaceId":boot.workspace_id,"accountId":boot.zuse_account_id,"generation":boot.runtime_generation,"enrollmentId":enrollment,"operationId":boot.extra["rustAllocationOperationId"],"storage":boot.runtime_storage,"checkout":boot.extra["rustRepositoryCheckout"]})
}
pub fn pending(data: &Path, boot: &CloudRuntimeBootstrap, enrollment: &str) -> Result<bool> {
    let path = data.join(FILE);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 8192,
        "runtime_preparation_identity_invalid"
    );
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    Ok(value["binding"] == binding(boot, enrollment) && value["state"] == "pending")
}
fn write(data: &Path, boot: &CloudRuntimeBootstrap, enrollment: &str, state: &str) -> Result<()> {
    if boot.launch_intent.is_none() {
        return Ok(());
    }
    let path = data.join(FILE);
    if path.exists() {
        ensure!(
            std::fs::symlink_metadata(&path)?.is_file(),
            "runtime_preparation_identity_invalid"
        );
    }
    let tmp = data.join(format!(
        ".preparation-ingress-{}",
        format!("{}-{}", std::process::id(), zuse_cloud_runtime::now_ms())
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(&serde_json::to_vec(
        &json!({"binding":binding(boot,enrollment),"state":state}),
    )?)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    std::fs::File::open(data)?.sync_all()?;
    Ok(())
}
pub fn begin(data: &Path, boot: &CloudRuntimeBootstrap, enrollment: &str) -> Result<()> {
    write(data, boot, enrollment, "pending")
}
pub fn complete(data: &Path, boot: &CloudRuntimeBootstrap, enrollment: &str) -> Result<()> {
    write(data, boot, enrollment, "completed")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_journal_fences_import_scope_and_never_stores_credentials_or_first_message() {
        let root = std::env::temp_dir().join(format!(
            "zuse-early-ingress-{}-{}",
            std::process::id(),
            zuse_cloud_runtime::now_ms()
        ));
        std::fs::create_dir(&root).unwrap();
        let boot:CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"task","zuseAccountId":"owner","chatId":"thread","initialSessionId":"session","runtimeCredential":"private-runtime-value","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"private-transcript-value","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"launchIntent":{"commandId":"launch:task","turnId":"turn:task","title":"Task","agent":"codex","model":"fixture","permissions":[],"firstMessage":"private-first-message"},"rustAllocationOperationId":"allocation","rustRepositoryCheckout":{"branch":"task/one","baseRef":"main","repositoryUrl":"https://github.com/owner/repo"}})).unwrap();
        assert!(!pending(&root, &boot, "enrollment").unwrap());
        begin(&root, &boot, "enrollment").unwrap();
        assert!(pending(&root, &boot, "enrollment").unwrap());
        let raw = std::fs::read_to_string(root.join(FILE)).unwrap();
        for secret in [
            "private-runtime-value",
            "private-transcript-value",
            "private-first-message",
        ] {
            assert!(!raw.contains(secret));
        }
        let mut changed = boot.clone();
        changed.runtime_generation += 1;
        assert!(!pending(&root, &changed, "enrollment").unwrap());
        let mut changed = boot.clone();
        changed.zuse_account_id = "other".into();
        assert!(!pending(&root, &changed, "enrollment").unwrap());
        assert!(!pending(&root, &boot, "other-enrollment").unwrap());
        complete(&root, &boot, "enrollment").unwrap();
        assert!(!pending(&root, &boot, "enrollment").unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(root.join(FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_file(root.join(FILE)).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
