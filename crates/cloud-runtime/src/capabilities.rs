//! Public implementation receipt from the exact guest binary, without enrollment.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandAdmissionReceipt {
    pub version: u32,
    pub runtime_sha256: String,
    pub command_admission_version: u32,
    pub runtime_rpc_protocol: String,
    pub storage_format: String,
    pub session_command_version: u32,
    pub execution_progress_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_agent_version: Option<u32>,
}
fn binary_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= 512 * 1024 * 1024, "runtime_binary_too_large");
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
impl CommandAdmissionReceipt {
    pub fn valid(&self) -> bool {
        self.version == 1
            && self.command_admission_version == zuse_protocol::cloud_task_commands::VERSION
            && self.runtime_rpc_protocol == zuse_protocol::cloud_gateway::RUST_RPC
            && self.storage_format == zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
            && self.session_command_version == 1
            && self.execution_progress_version == 1
            && self
                .managed_agent_version
                .is_none_or(|version| version == 1)
            && self.runtime_sha256.len() == 64
            && self.runtime_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    }
    pub fn matches_binary(&self, path: &Path) -> bool {
        self.valid()
            && binary_sha256(path)
                .is_ok_and(|digest| digest.eq_ignore_ascii_case(&self.runtime_sha256))
    }
}
pub fn current() -> Result<CommandAdmissionReceipt> {
    ensure!(cfg!(feature = "engine"), "runtime_execution_unavailable");
    Ok(CommandAdmissionReceipt {
        version: 1,
        runtime_sha256: binary_sha256(&std::env::current_exe()?)?,
        command_admission_version: zuse_protocol::cloud_task_commands::VERSION,
        runtime_rpc_protocol: zuse_protocol::cloud_gateway::RUST_RPC.into(),
        storage_format: zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT.into(),
        session_command_version: 1,
        execution_progress_version: 1,
        managed_agent_version: Some(1),
    })
}
/// Tests/non-guest executables cannot attest capability merely by linking this
/// library: the executable must implement the explicit receipt entrypoint.
pub(crate) fn probe_current() -> Option<CommandAdmissionReceipt> {
    let executable = std::env::current_exe().ok()?;
    let expected = binary_sha256(&executable).ok()?;
    let mut child = Command::new(&executable)
        .arg("runtime-capabilities")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()?
        .take(8193)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 8192 {
        return None;
    }
    let receipt: CommandAdmissionReceipt = serde_json::from_slice(&bytes).ok()?;
    (receipt.valid()
        && receipt.runtime_sha256.eq_ignore_ascii_case(&expected)
        && receipt.matches_binary(&executable))
    .then_some(receipt)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_receipt_serialization_stays_exact_and_agent_support_is_explicit() {
        let original = format!(
            "{{\"version\":1,\"runtimeSha256\":\"{}\",\"commandAdmissionVersion\":2,\"runtimeRpcProtocol\":\"{}\",\"storageFormat\":\"{}\",\"sessionCommandVersion\":1,\"executionProgressVersion\":1}}",
            "a".repeat(64),
            zuse_protocol::cloud_gateway::RUST_RPC,
            zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT,
        );
        let mut receipt: CommandAdmissionReceipt = serde_json::from_str(&original).unwrap();
        assert_eq!(receipt.managed_agent_version, None);
        assert_eq!(serde_json::to_string(&receipt).unwrap(), original);
        assert!(receipt.valid());
        receipt.managed_agent_version = Some(1);
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(value["managedAgentVersion"], 1);
        assert_eq!(
            serde_json::from_value::<CommandAdmissionReceipt>(value).unwrap(),
            receipt
        );
        receipt.managed_agent_version = Some(2);
        assert!(!receipt.valid());
    }
    #[test]
    fn linked_library_does_not_fabricate_a_guest_receipt() {
        // The test runner does not implement the guest's public entrypoint.
        assert!(probe_current().is_none());
    }
    #[cfg(feature = "engine")]
    #[test]
    fn receipt_binds_exact_binary_and_all_required_protocols() {
        let receipt = current().unwrap();
        assert!(receipt.valid());
        assert!(receipt.matches_binary(&std::env::current_exe().unwrap()));
        let mut changed = receipt.clone();
        changed.execution_progress_version = 0;
        assert!(!changed.valid());
        let mut changed = receipt;
        changed.runtime_sha256 = "0".repeat(64);
        assert!(!changed.matches_binary(&std::env::current_exe().unwrap()));
    }
}
