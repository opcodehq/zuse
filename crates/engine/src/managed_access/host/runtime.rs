//! One connector task per host, gated by listener, account and durable link state.
use super::super::{Error, Lease, Result};
use super::{Host, lock, write_saved};
use serde::Deserialize;
use serde_json::json;
use std::{
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;
struct Config {
    environment: String,
    credential: Zeroizing<String>,
    connector: Zeroizing<String>,
    port: u16,
    lease: Lease,
    retired: CancellationToken,
    listener: CancellationToken,
}
fn snapshot(host: &Host) -> Option<Config> {
    let account_cancel = host.0.client.account.session_cancellation();
    if account_cancel.is_cancelled() {
        return None;
    }
    let account = host.0.client.account.status().user?;
    let mut state = lock(&host.0.state);
    let (port, listener) = state.listener.as_ref()?.clone();
    if listener.is_cancelled() {
        return None;
    }
    let saved = state.saved.as_mut()?;
    if !saved.enabled || saved.subject != account.id {
        return None;
    }
    // The API reconciles the tunnel origin from the heartbeat before launch.
    if saved.local_port != port {
        saved.local_port = port;
        if write_saved(&host.0.root, saved).is_err() {
            state.runtime_error = Some(
                "The browser listener changed but managed access settings could not be saved."
                    .into(),
            );
            return None;
        }
    }
    Some(Config {
        environment: saved.environment_id.clone(),
        credential: Zeroizing::new(saved.environment_credential.clone()),
        connector: Zeroizing::new(saved.connector_token.clone()),
        port,
        lease: Lease {
            subject: account.id,
            cancel: account_cancel,
        },
        retired: state.retired.clone(),
        listener,
    })
}
fn error(host: &Host, message: Option<&str>) {
    lock(&host.0.state).runtime_error = message.map(str::to_owned);
}
fn disable(host: &Host, message: &str) {
    let mut state = lock(&host.0.state);
    let persisted = state.saved.as_mut().is_some_and(|saved| {
        saved.enabled = false;
        write_saved(&host.0.root, saved).is_ok()
    });
    state.retired.cancel();
    state.tunnel_running = false;
    state.runtime_error = Some(if persisted {
        message.into()
    } else {
        "Managed access stopped, but its disabled state could not be saved. Fix profile storage before restarting.".into()
    });
    host.0.changed.send_replace(());
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Heartbeat {
    machine_action: Option<String>,
}
async fn beat(host: &Host, config: &Config) -> Result<()> {
    let mut features = zuse_protocol::capabilities::current();
    features.push(zuse_serve::managed::PROTOCOL.into());
    let body = json!({
        "origin":{"localHttpHost":"127.0.0.1","localHttpPort":config.port},
        "runtimeVersion":env!("CARGO_PKG_VERSION"),
        "wireProtocolVersion":zuse_serve::managed::WIRE_VERSION,
        "capabilities":{"version":1,"features":features},"serviceState":"healthy"
    });
    let response: Heartbeat = host
        .0
        .client
        .send(
            &config.lease,
            reqwest::Method::POST,
            &format!("/v1/environments/{}/heartbeat", config.environment),
            Some(&body),
            &format!("Bearer {}", config.credential.as_str()),
            None,
        )
        .await?;
    if response.machine_action.is_some() {
        disable(
            host,
            "The account service requested credential cleanup. Managed access is disabled; review this computer before linking it again.",
        );
        return Err(Error::Changed);
    }
    lock(&host.0.state).last_heartbeat = Some(chrono::Utc::now().timestamp_millis());
    Ok(())
}
fn backoff(failures: u32) -> Duration {
    Duration::from_secs((1u64 << failures.min(6)).min(60))
}
async fn heartbeat(host: &Host, config: &Config, stop: CancellationToken) {
    let mut failures = 0;
    loop {
        let delay = if failures == 0 {
            Duration::from_millis(
                (lock(&host.0.state).last_heartbeat.unwrap_or(0) + 30000
                    - chrono::Utc::now().timestamp_millis())
                .max(0) as u64,
            )
        } else {
            backoff(failures)
        };
        // A resume advances wall time while a monotonic timer may remain frozen.
        let until = chrono::Utc::now().timestamp_millis() + delay.as_millis() as i64;
        let started = Instant::now();
        loop {
            if started.elapsed() >= delay || chrono::Utc::now().timestamp_millis() >= until {
                break;
            }
            tokio::select! {biased;_=stop.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
        }
        let result =
            tokio::select! {biased;_=stop.cancelled()=>return, result=beat(host,config)=>result};
        match result {
            Ok(()) => {
                failures = 0;
                error(host, None);
            }
            Err(Error::SignedOut | Error::Rejected) => {
                disable(
                    host,
                    "The account service no longer authorizes this computer. Unlink it before linking again.",
                );
                return;
            }
            Err(Error::Changed) => return,
            Err(_) => {
                failures += 1;
                error(
                    host,
                    Some(
                        "The account service is unavailable. The connector will keep its current connection while health reporting retries.",
                    ),
                );
            }
        }
    }
}
fn token_file(root: &std::path::Path, token: &str) -> Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let directory = root.join("managed-access");
    std::fs::create_dir_all(&directory).map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Storage)?;
    }
    let mut file = tempfile::Builder::new()
        .prefix("connector-")
        .rand_bytes(12)
        .tempfile_in(&directory)
        .map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Error::Storage)?;
    }
    file.write_all(token.as_bytes())
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| Error::Storage)?;
    Ok(file)
}
async fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // This is an owned, unreaped Child, never a PID found by scanning processes.
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    let _ = child.start_kill();
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
async fn connector(
    host: &Host,
    binary: PathBuf,
    token: tempfile::NamedTempFile,
    stop: CancellationToken,
) {
    let mut failures = 0;
    loop {
        if stop.is_cancelled() {
            break;
        }
        let started = Instant::now();
        let mut command = tokio::process::Command::new(&binary);
        #[cfg(target_os = "linux")]
        unsafe {
            let parent = libc::getpid();
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
                }
                Ok(())
            });
        }
        let spawn = command
            .env_remove("TUNNEL_TOKEN")
            .env_remove("TUNNEL_TOKEN_FILE")
            .args(["tunnel", "--no-autoupdate", "run", "--token-file"])
            .arg(token.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        match spawn {
            Ok(mut child) => {
                lock(&host.0.state).tunnel_running = true;
                tokio::select! {biased;
                    _=stop.cancelled()=>{terminate(&mut child).await;break;},
                    _=child.wait()=>{},
                }
            }
            Err(_) => {}
        }
        lock(&host.0.state).tunnel_running = false;
        if started.elapsed() > Duration::from_secs(60) {
            failures = 0;
        }
        failures += 1;
        error(
            host,
            Some("The managed connector stopped. Restarting it with a bounded delay."),
        );
        tokio::select! {biased;_=stop.cancelled()=>break,_=tokio::time::sleep(backoff(failures))=>{}}
    }
    lock(&host.0.state).tunnel_running = false;
    // Drop removes this launch's credential file only after its child has exited.
    drop(token);
}
async fn ensure_binary(host: &Host) -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = lock(&host.0.state).fixture_binary.clone() {
        return Ok(path);
    }
    super::install::ensure(&host.0.root).await
}
async fn active(host: &Host, config: &Config, stop: CancellationToken) {
    // Reconcile the exact live listener and check revocation before any exposure.
    let mut failures = 0;
    loop {
        let result =
            tokio::select! {biased;_=stop.cancelled()=>return,result=beat(host,config)=>result};
        match result {
            Ok(()) => break,
            Err(Error::SignedOut | Error::Rejected) => {
                disable(
                    host,
                    "The account service no longer authorizes this computer. Unlink it before linking again.",
                );
                return;
            }
            Err(Error::Changed) => return,
            Err(_) => {
                failures += 1;
                error(
                    host,
                    Some("Waiting for the account service before exposing this computer."),
                );
            }
        }
        tokio::select! {biased;_=stop.cancelled()=>return,_=tokio::time::sleep(backoff(failures))=>{}}
    }
    let mut failures = 0;
    let binary = loop {
        let result =
            tokio::select! {biased;_=stop.cancelled()=>return,result=ensure_binary(host)=>result};
        match result {
            Ok(binary) => break binary,
            Err(_) => {
                failures += 1;
                error(
                    host,
                    Some(
                        "The verified managed connector could not be installed. Retrying when connected.",
                    ),
                );
            }
        }
        tokio::select! {biased;_=stop.cancelled()=>return,_=tokio::time::sleep(backoff(failures))=>{}}
    };
    let mut failures = 0;
    let token = loop {
        match token_file(&host.0.root, &config.connector) {
            Ok(token) => break token,
            Err(_) => {
                failures += 1;
                error(
                    host,
                    Some(
                        "The connector credential file could not be saved. Retrying after a bounded delay.",
                    ),
                );
            }
        }
        tokio::select! {biased;_=stop.cancelled()=>return,_=tokio::time::sleep(backoff(failures))=>{}}
    };
    error(host, None);
    tokio::join!(
        connector(host, binary, token, stop.clone()),
        heartbeat(host, config, stop)
    );
}
pub(super) async fn supervise(host: Host) {
    loop {
        let result = tokio::select! {biased;_=host.0.closed.cancelled()=>return,result=super::orphans::cleanup(&host.0.root)=>result};
        if result.is_ok() {
            break;
        }
        error(
            &host,
            Some(
                "An earlier connector could not be retired. Waiting before starting another connector.",
            ),
        );
        tokio::select! {biased;_=host.0.closed.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(60))=>{}}
    }
    let mut changed = host.0.changed.subscribe();
    let mut account = host.0.client.account.watch();
    loop {
        changed.borrow_and_update();
        account.borrow_and_update();
        if host.0.closed.is_cancelled() {
            break;
        }
        if let Some(config) = snapshot(&host) {
            let stop = CancellationToken::new();
            let task = active(&host, &config, stop.clone());
            tokio::pin!(task);
            let finished = tokio::select! {biased;
                _=host.0.closed.cancelled()=>false,
                _=config.lease.cancel.cancelled()=>false,
                _=config.retired.cancelled()=>false,
                _=config.listener.cancelled()=>false,
                _=changed.changed()=>false,
                _=&mut task=>true,
            };
            stop.cancel();
            if !finished {
                let _ = tokio::time::timeout(Duration::from_secs(4), &mut task).await;
            }
            lock(&host.0.state).tunnel_running = false;
            if finished && !config.retired.is_cancelled() && !host.0.closed.is_cancelled() {
                tokio::select! {biased;_=host.0.closed.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(5))=>{}}
            }
        } else {
            tokio::select! {biased;
                _=host.0.closed.cancelled()=>break,
                _=changed.changed()=>{},
                _=account.changed()=>{},
            }
        }
    }
}
