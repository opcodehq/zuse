//! Opt-in live check of one provider adapter against a published managed base:
//! create, installed runtime command, disk marker, sleep, resume, marker and
//! runtime again, delete and confirmed absence. No account enrollment, model,
//! repository or user data. Every step's identity is written to a private
//! receipt before the next mutation so an uncertain outcome can be reconciled.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use zuse_cloud::providers::{
    artifact::ArtifactBinding,
    contract::{Allocation, OperationState, Provider, ProviderOperation},
    registry::Registry,
};
use zuse_protocol::cloud_computers::ProviderId;

const RUNTIME: &str = "/usr/local/bin/zuse-cloud-runtime --version";
const MARKER: &str = "/tmp/zuse-lifecycle-probe-marker";

struct Receipt {
    path: PathBuf,
    value: Value,
}
impl Receipt {
    fn record(&mut self, key: &str, value: Value) -> Result<()> {
        self.value[key] = value;
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.value)?)?;
        Ok(())
    }
}

async fn command(
    provider: &dyn Provider,
    machine: &str,
    line: &str,
    wait: Duration,
) -> Result<(u64, String)> {
    let started = Instant::now();
    let mut last = String::from("no attempt");
    while started.elapsed() < wait {
        match provider.execute(machine, line).await {
            Ok(result) if result.exit_code == 0 => {
                return Ok((
                    started.elapsed().as_millis() as u64,
                    String::from_utf8_lossy(&result.stdout)
                        .trim()
                        .chars()
                        .take(200)
                        .collect(),
                ));
            }
            Ok(result) => last = format!("exit {}", result.exit_code),
            Err(error) => last = format!("{error:?}"),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!("command_not_ready after {}ms: {last}", wait.as_millis())
}

async fn settle(
    provider: &dyn Provider,
    receipt: &ProviderOperation,
    wait: Duration,
) -> Result<u64> {
    let started = Instant::now();
    let mut last = String::from("no observation");
    while started.elapsed() < wait {
        match provider.operation(receipt).await {
            Ok(OperationState::Completed) => return Ok(started.elapsed().as_millis() as u64),
            Ok(OperationState::Superseded) => bail!("operation_superseded"),
            Ok(state) => last = format!("{state:?}"),
            Err(error) => last = format!("{error:?}"),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("operation_not_settled after {}ms: {last}", wait.as_millis())
}

async fn lifecycle(
    provider: &dyn Provider,
    machine: &str,
    receipt: &mut Receipt,
    candidate: Option<&[u8]>,
) -> Result<()> {
    let (ms, version) = command(provider, machine, RUNTIME, Duration::from_secs(120)).await?;
    receipt.record("runtimeReady", json!({"ms":ms,"version":version}))?;
    // What app features rely on inside the computer. Recorded, never fatal.
    let mut checks = serde_json::Map::new();
    for (name, line) in [
        ("user", "id -un"),
        ("sshd", "test -x /usr/sbin/sshd"),
        ("sshKeygen", "command -v ssh-keygen"),
        ("sudoNoPassword", "sudo -n true"),
        ("procStat", "head -c 64 /proc/stat"),
        ("memInfo", "grep -m1 MemTotal /proc/meminfo"),
        ("rootDisk", "df -P / | tail -1"),
        ("git", "git --version"),
        ("nproc", "nproc"),
        ("cgroupMemoryMax", "cat /sys/fs/cgroup/memory.max"),
        ("cgroupMemoryCurrent", "cat /sys/fs/cgroup/memory.current"),
        (
            "cgroupMemoryStat",
            "grep -E '^(anon|file|inactive_file) ' /sys/fs/cgroup/memory.stat | tr '\\n' ' '",
        ),
        ("cgroupCpuMax", "cat /sys/fs/cgroup/cpu.max"),
        ("cgroupCpuStat", "head -1 /sys/fs/cgroup/cpu.stat"),
        (
            "cgroupV1Memory",
            "cat /sys/fs/cgroup/memory/memory.limit_in_bytes",
        ),
        ("homeDisk", "df -P \"$HOME\" | tail -1"),
        (
            "mounts",
            "grep -vE ' (proc|sysfs|cgroup2?|devpts|mqueue|tmpfs|devtmpfs) ' /proc/mounts | cut -d' ' -f2,3 | tr '\\n' ';'",
        ),
    ] {
        let value = match provider.execute(machine, line).await {
            Ok(result) => json!({"exit":result.exit_code,
                "out":String::from_utf8_lossy(&result.stdout).trim().chars().take(120).collect::<String>()}),
            Err(error) => json!({"error":format!("{error:?}")}),
        };
        checks.insert(name.into(), value);
    }
    receipt.record("checks", Value::Object(checks))?;
    if let Some(bytes) = candidate {
        // The current guest runtime, run as the image's own user, beside the
        // base's installed one. It only reports its version and capabilities.
        let started = Instant::now();
        // Adapters accept at most 1 MiB per upload; join the parts on the machine.
        for (index, part) in bytes.chunks(1024 * 1024).enumerate() {
            provider
                .upload(machine, &format!("/tmp/zuse-runtime-part-{index:05}"), part)
                .await
                .map_err(|e| anyhow::anyhow!("candidate upload part {index}: {e:?}"))?;
        }
        command(
            provider,
            machine,
            "cat /tmp/zuse-runtime-part-* > /tmp/zuse-runtime-candidate && rm -f /tmp/zuse-runtime-part-*",
            Duration::from_secs(60),
        )
        .await
        .context("join candidate runtime")?;
        let upload_ms = started.elapsed().as_millis() as u64;
        let (_, version) = command(
            provider,
            machine,
            "chmod 0755 /tmp/zuse-runtime-candidate && /tmp/zuse-runtime-candidate --version",
            Duration::from_secs(30),
        )
        .await
        .context("candidate runtime --version")?;
        let (_, capabilities) = command(
            provider,
            machine,
            "/tmp/zuse-runtime-candidate runtime-capabilities",
            Duration::from_secs(30),
        )
        .await
        .context("candidate runtime-capabilities")?;
        receipt.record(
            "candidateRuntime",
            json!({"bytes":bytes.len(),"uploadMs":upload_ms,"version":version,"capabilities":capabilities}),
        )?;
    }
    command(
        provider,
        machine,
        &format!("echo retained > {MARKER} && sync"),
        Duration::from_secs(20),
    )
    .await?;

    let started = Instant::now();
    let sleep = provider
        .sleep(machine)
        .await
        .map_err(|e| anyhow::anyhow!("sleep request {e:?}"))?;
    let request_ms = started.elapsed().as_millis() as u64;
    let settled_ms = settle(provider, &sleep, Duration::from_secs(180))
        .await
        .context("sleep")?;
    receipt.record(
        "sleep",
        json!({"requestMs":request_ms,"settledMs":settled_ms}),
    )?;

    let started = Instant::now();
    let resume = provider
        .resume(machine, 1800)
        .await
        .map_err(|e| anyhow::anyhow!("resume request {e:?}"))?;
    let request_ms = started.elapsed().as_millis() as u64;
    let settled_ms = settle(provider, &resume, Duration::from_secs(180))
        .await
        .context("resume")?;
    let (ms, _) = command(provider, machine, RUNTIME, Duration::from_secs(120))
        .await
        .context("runtime after resume")?;
    let (_, marker) = command(
        provider,
        machine,
        &format!("cat {MARKER}"),
        Duration::from_secs(20),
    )
    .await
    .context("disk marker after resume")?;
    ensure!(marker == "retained", "disk_marker_lost");
    receipt.record(
        "resume",
        json!({"requestMs":request_ms,"settledMs":settled_ms,"runtimeAfterResumeMs":ms,"diskRetained":true,
               "totalToRuntimeMs":started.elapsed().as_millis() as u64}),
    )?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    ensure!(
        args.first().map(String::as_str) == Some("--live"),
        "Usage: provider_lifecycle_probe --live --provider boat|boxd|e2b|daytona --artifact ID [--binding JSON] [--size default] [--runtime PATH]"
    );
    let id = match value("--provider").as_deref() {
        Some("boat") => ProviderId::Boat,
        Some("boxd") => ProviderId::Boxd,
        Some("e2b") => ProviderId::E2b,
        Some("daytona") => ProviderId::Daytona,
        _ => bail!("--provider must be boat, boxd, e2b or daytona"),
    };
    let artifact = value("--artifact").context("--artifact required")?;
    let binding: Option<ArtifactBinding> = value("--binding")
        .map(|raw| serde_json::from_str(&raw))
        .transpose()?;
    let size = value("--size").unwrap_or_else(|| "default".into());
    let candidate = value("--runtime")
        .map(std::fs::read)
        .transpose()
        .context("--runtime")?;
    let registry = Registry::from_environment();
    let provider = registry
        .get(id)
        .with_context(|| format!("{} is not configured", id.as_str()))?;

    let name = format!(
        "zuse-probe-{}-{}",
        id.as_str(),
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let root = PathBuf::from(".context/provider-lifecycle");
    std::fs::create_dir_all(&root)?;
    let mut receipt = Receipt {
        path: root.join(format!("{name}.json")),
        value: json!({"provider":id.as_str(),"name":name,"artifact":artifact,"size":size,"phase":"creating"}),
    };
    receipt.record("startedAt", json!(chrono::Utc::now().to_rfc3339()))?;

    let started = Instant::now();
    let created = provider
        .allocate(&Allocation {
            artifact: &artifact,
            artifact_binding: binding.as_ref(),
            name: &name,
            operation_id: &name,
            size: &size,
            lifetime_seconds: 1800,
        })
        .await;
    let create_ms = started.elapsed().as_millis() as u64;
    let machine = match created {
        Ok(machine) => machine,
        Err(error) => match provider.find_allocation(&name).await {
            // The create may have happened even though its response was lost.
            Ok(Some(machine)) => machine,
            Ok(None) => {
                receipt.record(
                    "create",
                    json!({"ms":create_ms,"error":format!("{error:?}"),"absent":true}),
                )?;
                bail!("create failed: {error:?} (confirmed absent)");
            }
            Err(lookup) => {
                receipt.record("create", json!({"ms":create_ms,"error":format!("{error:?}"),"lookup":format!("{lookup:?}")}))?;
                bail!("create outcome unknown: {error:?}; reconcile {name}");
            }
        },
    };
    receipt.record("create", json!({"ms":create_ms,"machineId":machine.id}))?;
    eprintln!("created {} ({}) in {create_ms}ms", machine.id, name);

    let outcome = lifecycle(
        provider.as_ref(),
        &machine.id,
        &mut receipt,
        candidate.as_deref(),
    )
    .await;
    if let Err(error) = &outcome {
        receipt.record("lifecycleError", json!(format!("{error:#}")))?;
    }

    // Always clean up, even after a failed step.
    let deleted = match provider.delete(&machine.id).await {
        Ok(operation) => settle(provider.as_ref(), &operation, Duration::from_secs(180))
            .await
            .map(|_| ()),
        Err(error) => Err(anyhow::anyhow!("delete request {error:?}")),
    };
    let absent = matches!(provider.inspect(&machine.id).await, Ok(None))
        || matches!(provider.find_allocation(&name).await, Ok(None));
    receipt.record(
        "delete",
        json!({"settled":deleted.is_ok(),"error":deleted.as_ref().err().map(|e| format!("{e:#}")),"absent":absent}),
    )?;
    println!("{}", serde_json::to_string_pretty(&receipt.value)?);
    outcome?;
    ensure!(
        absent,
        "cleanup not confirmed; reconcile {} ({name})",
        machine.id
    );
    Ok(())
}
