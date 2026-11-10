//! Declared startup services belong to one retained computer, never to the UI process.
use super::*;
use tokio::io::AsyncWriteExt;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ServiceReceipt {
    workspace: String,
    incarnation: String,
    recipe_digest: String,
    boot_id: String,
    pid: u32,
    start_ticks: u64,
}
pub struct Services {
    children: Vec<CommandGroup>,
    logs: Vec<tokio_util::task::AbortOnDropHandle<()>>,
}
impl Drop for Services {
    fn drop(&mut self) {
        // Stop groups before closing their log pipes.
        for child in &mut self.children {
            child.terminate_descendants();
        }
        self.logs.clear();
    }
}
fn process_start(pid: u32) -> Result<Option<u64>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| anyhow::anyhow!("environment_process_identity_invalid"))?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    if fields.first() == Some(&"Z") {
        return Ok(None);
    }
    Ok(Some(
        fields
            .get(19)
            .ok_or_else(|| anyhow::anyhow!("environment_process_identity_invalid"))?
            .parse()?,
    ))
}
async fn stop_previous(path: &Path, prepared: &PreparedEnvironment, boot_id: &str) -> Result<()> {
    let previous = match std::fs::read(path) {
        Ok(bytes) => {
            ensure!(bytes.len() < 4096, "environment_service_receipt_invalid");
            serde_json::from_slice::<ServiceReceipt>(&bytes)?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        previous.workspace == prepared.workspace
            && previous.incarnation == prepared.incarnation
            && previous.recipe_digest == prepared.digest,
        "environment_service_identity_mismatch"
    );
    if previous.boot_id != boot_id || process_start(previous.pid)? != Some(previous.start_ticks) {
        return Ok(());
    }
    // This is our previous runtime's process group, verified against both boot and
    // process start identity. A recycled PID is never signalled.
    unsafe {
        libc::kill(-(previous.pid as i32), libc::SIGTERM);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while process_start(previous.pid)? == Some(previous.start_ticks) {
        if tokio::time::Instant::now() >= deadline {
            unsafe {
                libc::kill(-(previous.pid as i32), libc::SIGKILL);
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}
async fn pump(
    mut input: impl AsyncRead + Unpin,
    file: PathBuf,
    secrets: std::sync::Arc<PrivateEnvironment>,
    protection: OutputProtection,
) {
    let mut private = protection.stream();
    let mut redactor = Redactor::new(&secrets);
    let Ok(mut output) = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(file)
        .await
    else {
        return;
    };
    let mut written = 0;
    let mut chunk = [0u8; 4096];
    while let Ok(read) = input.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        let clean = private.push(&redactor.push(&chunk[..read], false), false);
        let keep = clean.len().min(MAX_LOG - written);
        if keep > 0 {
            if output.write_all(&clean[..keep]).await.is_err() {
                break;
            }
            written += keep;
        }
    }
    let tail = private.push(&redactor.push(&[], true), true);
    let keep = tail.len().min(MAX_LOG - written);
    if keep > 0 {
        let _ = output.write_all(&tail[..keep]).await;
    }
}
async fn port_ready(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(100),
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// A process-loss restart reconciles only this task's recorded services. A memory
/// resume keeps this guard and the existing processes without running setup again.
pub async fn start_services(data: &Path, prepared: &PreparedEnvironment) -> Result<Services> {
    start_services_with_environment(
        data,
        prepared,
        std::sync::Arc::new(PrivateEnvironment::new()),
    )
    .await
}
pub async fn start_services_with_environment(
    data: &Path,
    prepared: &PreparedEnvironment,
    secrets: std::sync::Arc<PrivateEnvironment>,
) -> Result<Services> {
    start_services_with_protection(data, prepared, secrets, OutputProtection::default()).await
}
pub async fn start_services_with_protection(
    data: &Path,
    prepared: &PreparedEnvironment,
    secrets: std::sync::Arc<PrivateEnvironment>,
    protection: OutputProtection,
) -> Result<Services> {
    prepared.recipe.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        prepared.state == "ready",
        "environment_preparation_not_ready"
    );
    let mut services = Services {
        children: vec![],
        logs: vec![],
    };
    if prepared.recipe.startup.is_empty() {
        return Ok(services);
    }
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    for service in &prepared.recipe.startup {
        let receipt_path = data.join(format!("startup-{}.json", service.name));
        stop_previous(&receipt_path, prepared, &boot_id).await?;
        if let Some(port) = service.port {
            ensure!(!port_ready(port).await, "environment_service_port_in_use");
        }
        let mut command = environment_command(&prepared.checkout, &prepared.recipe);
        command.envs(secrets.iter().map(|(name, value)| (name, value.as_str())));
        // The script cannot run until its process receipt is committed. If the
        // runtime dies in that window the pipe closes and the gate exits.
        command.args(["--noprofile", "--norc", "-c",
            "IFS= read -r permit; [ \"$permit\" = start ] || exit 1; exec /bin/bash --noprofile --norc -e -o pipefail -c \"$1\"",
            "zuse-startup", &service.command]).stdin(Stdio::piped());
        let child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("environment_service_spawn_failed"))?;
        let mut child = CommandGroup {
            child,
            pid: Some(pid),
        };
        let receipt = ServiceReceipt {
            workspace: prepared.workspace.clone(),
            incarnation: prepared.incarnation.clone(),
            recipe_digest: prepared.digest.clone(),
            boot_id: boot_id.clone(),
            pid,
            start_ticks: process_start(pid)?
                .ok_or_else(|| anyhow::anyhow!("environment_service_spawn_failed"))?,
        };
        atomic_write(&receipt_path, &serde_json::to_vec(&receipt)?)?;
        let stdout = child.child.stdout.take().expect("stdout piped");
        let stderr = child.child.stderr.take().expect("stderr piped");
        services
            .logs
            .push(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                pump(
                    stdout,
                    data.join(format!("startup-{}.stdout.log", service.name)),
                    secrets.clone(),
                    protection.clone(),
                ),
            )));
        services
            .logs
            .push(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                pump(
                    stderr,
                    data.join(format!("startup-{}.stderr.log", service.name)),
                    secrets.clone(),
                    protection.clone(),
                ),
            )));
        child
            .child
            .stdin
            .take()
            .expect("stdin piped")
            .write_all(b"start\n")
            .await?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(service.timeout_seconds.into());
        loop {
            ensure!(
                child.child.try_wait()?.is_none(),
                "environment_service_exited"
            );
            if let Some(port) = service.port {
                if port_ready(port).await {
                    break;
                }
            } else {
                break;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "environment_service_readiness_timeout"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        services.children.push(child);
    }
    Ok(services)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_protocol::cloud_computers::StartupService;

    #[tokio::test]
    async fn service_readiness_process_loss_and_shutdown_preserve_installed_files() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let checkout = root.path().join("checkout");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&checkout).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut recipe = EnvironmentRecipe::default();
        recipe.scripts.initialize = Some("printf installed > package-state".into());
        recipe.startup.push(StartupService {
            name: "web".into(),
            command: format!("python3 -u -m http.server {port} --bind 127.0.0.1"),
            port: Some(port),
            timeout_seconds: 5,
        });
        let prepared = super::super::prepare(&data, &checkout, "task", "disk", &recipe)
            .await
            .unwrap();
        let mut services = start_services(&data, &prepared).await.unwrap();
        assert!(port_ready(port).await);
        let original_pid = services.children[0].pid.unwrap();
        // Simulate loss of the supervisor without signalling its child. The next
        // process must reconcile the persisted identity before replacing it.
        let mut orphan = services.children.remove(0);
        drop(services);
        let replacement = start_services(&data, &prepared).await.unwrap();
        assert!(process_start(original_pid).unwrap().is_none());
        orphan.child.wait().await.unwrap();
        orphan.pid = None;
        drop(orphan);
        assert!(port_ready(port).await);
        drop(replacement);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!port_ready(port).await);
        assert_eq!(
            std::fs::read(checkout.join("package-state")).unwrap(),
            b"installed"
        );
    }

    #[tokio::test]
    async fn recycled_or_unrelated_pid_is_never_stopped_from_an_old_receipt() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        let data = root.path().join("data");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::create_dir(&data).unwrap();
        let prepared = super::super::prepare(
            &data,
            &checkout,
            "task",
            "disk",
            &EnvironmentRecipe::default(),
        )
        .await
        .unwrap();
        let mut unrelated = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = unrelated.id().unwrap();
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .unwrap()
            .trim()
            .to_owned();
        let path = data.join("startup-old.json");
        let receipt = ServiceReceipt {
            workspace: prepared.workspace.clone(),
            incarnation: prepared.incarnation.clone(),
            recipe_digest: prepared.digest.clone(),
            boot_id: boot_id.clone(),
            pid,
            start_ticks: process_start(pid).unwrap().unwrap() + 1,
        };
        atomic_write(&path, &serde_json::to_vec(&receipt).unwrap()).unwrap();
        stop_previous(&path, &prepared, &boot_id).await.unwrap();
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().await.unwrap();
    }
}

#[cfg(all(test, feature = "engine"))]
mod secret_tests {
    use super::*;
    #[tokio::test]
    async fn service_pipe_masks_registered_rotated_grant_across_chunks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("service.log");
        let registry =
            std::sync::Arc::new(zuse_providers::credentials::redaction::Secrets::default());
        registry.record("old-grant").unwrap();
        let (mut writer, reader) = tokio::io::duplex(128);
        let output = path.clone();
        let protection = OutputProtection::new(registry.clone());
        let task = tokio::spawn(async move {
            pump(
                reader,
                output,
                std::sync::Arc::new(PrivateEnvironment::new()),
                protection,
            )
            .await
        });
        writer.write_all(b"normal old-").await.unwrap();
        tokio::task::yield_now().await;
        writer.write_all(b"grant ").await.unwrap();
        registry.record("new-grant").unwrap();
        writer.write_all(b"new-").await.unwrap();
        tokio::task::yield_now().await;
        writer.write_all(b"grant end").await.unwrap();
        writer.shutdown().await.unwrap();
        task.await.unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "normal [redacted] [redacted] end"
        );
    }
}
