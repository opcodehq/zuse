//! Internal-pilot transport repair for retained native Codex sessions. This
//! never submits a prompt, rotates an account or restarts a process.
use crate::grants::ProviderBroker;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};
use zuse_protocol::cloud::CloudRuntimeBootstrap;
use zuse_providers::codex::memory::{IdleNativeProcess, ManagedMemorySessions};

const ERROR: &str = "codex_memory_transport_recovery_required";
pub struct PreparedTransport {
    registry: Arc<ManagedMemorySessions>,
    processes: Vec<IdleNativeProcess>,
    snapshots: Vec<String>,
    workspace: String,
    generation: i64,
    disk: String,
}
fn binding(boot: &CloudRuntimeBootstrap) -> Result<Value> {
    Ok(
        json!({"version":1,"workspaceId":boot.workspace_id,"generation":boot.runtime_generation,"storageIncarnationId":boot.runtime_storage.as_ref().ok_or_else(||anyhow::anyhow!(ERROR))?.storage_incarnation_id}),
    )
}
async fn helper(action: &str, request: &Value) -> Result<Value> {
    let exe = std::fs::canonicalize(std::env::current_exe()?)?;
    let encoded = serde_json::to_vec(request)?;
    ensure!(encoded.len() <= 65_536, ERROR);
    let mut cmd = Command::new("/usr/bin/sudo");
    cmd.arg("-n")
        .arg("--")
        .arg(exe)
        .arg("native-sockets")
        .arg(action)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|_| anyhow::anyhow!(ERROR))?;
    let mut input = child.stdin.take().ok_or_else(|| anyhow::anyhow!(ERROR))?;
    input.write_all(&encoded).await?;
    drop(input);
    let output = tokio::time::timeout(Duration::from_secs(8), child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!(ERROR))??;
    ensure!(output.stdout.len() <= 131_072, ERROR);
    let value: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| anyhow::anyhow!(ERROR))?;
    ensure!(
        output.status.success()
            && value["version"] == 1
            && value["status"]
                == if action == "capture" {
                    "captured"
                } else {
                    "reset"
                },
        ERROR
    );
    Ok(value)
}
impl PreparedTransport {
    pub async fn prepare(
        boot: &CloudRuntimeBootstrap,
        broker: Option<&ProviderBroker>,
    ) -> Result<Option<Self>> {
        if boot.extra.get("internalCodexMemoryTransportReset") != Some(&json!(true)) {
            return Ok(None);
        }
        let registry = broker
            .ok_or_else(|| anyhow::anyhow!(ERROR))?
            .native_memory_sessions();
        let processes = registry
            .inspect_idle()
            .await
            .map_err(|_| anyhow::anyhow!(ERROR))?;
        let mut snapshots = Vec::with_capacity(processes.len());
        for process in &processes {
            let mut request = binding(boot)?;
            request["nativePid"] = json!(process.pid);
            let receipt = helper("capture", &request).await?;
            snapshots.push(
                receipt["snapshot"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 65_536)
                    .ok_or_else(|| anyhow::anyhow!(ERROR))?
                    .into(),
            );
        }
        ensure!(
            registry
                .inspect_idle()
                .await
                .map_err(|_| anyhow::anyhow!(ERROR))?
                == processes,
            ERROR
        );
        Ok(Some(Self {
            registry,
            processes,
            snapshots,
            workspace: boot.workspace_id.clone(),
            generation: boot.runtime_generation,
            disk: boot
                .runtime_storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!(ERROR))?
                .storage_incarnation_id
                .clone(),
        }))
    }
    pub async fn after_wake(self, boot: &CloudRuntimeBootstrap) -> Result<()> {
        ensure!(
            boot.extra.get("internalCodexMemoryTransportReset") == Some(&json!(true))
                && boot.workspace_id == self.workspace
                && boot.runtime_generation == self.generation
                && boot
                    .runtime_storage
                    .as_ref()
                    .is_some_and(|s| s.storage_incarnation_id == self.disk),
            ERROR
        );
        ensure!(
            self.registry
                .inspect_idle()
                .await
                .map_err(|_| anyhow::anyhow!(ERROR))?
                == self.processes,
            ERROR
        );
        for (process, snapshot) in self.processes.iter().zip(self.snapshots.iter()) {
            let mut request = binding(boot)?;
            request["nativePid"] = json!(process.pid);
            request["snapshot"] = json!(snapshot);
            helper("reset", &request).await?;
        }
        // Kernel abort is delivered asynchronously to the native WS pump. Let
        // it observe closure before admitting the next distinct command.
        tokio::time::sleep(Duration::from_millis(40)).await;
        ensure!(
            self.registry
                .inspect_idle()
                .await
                .map_err(|_| anyhow::anyhow!(ERROR))?
                == self.processes,
            ERROR
        );
        Ok(())
    }
}
