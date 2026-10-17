//! Private command supervisor. It receives specification, heartbeats and stdin
//! over one pipe and emits bounded output frames over another. No command or
//! credential is placed in the supervisor's arguments.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Mutex, mpsc, oneshot},
};
use tokio_util::codec::{FramedRead, LinesCodec};

pub const FLAG: &str = "--zuse-command-host";
pub const MAX_FRAME: usize = 512 * 1024;
#[derive(Serialize, Deserialize)]
pub(crate) struct Specification {
    pub file: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Option<BTreeMap<String, String>>,
    pub timeout_ms: u64,
    pub lease: PathBuf,
}
pub fn requested() -> bool {
    std::env::args_os().nth(1).is_some_and(|arg| arg == FLAG)
}
pub fn run_blocking() -> io::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid command supervisor frame",
    )
}
#[cfg(unix)]
fn kill_group(group: i32) {
    // The parent assigns a fresh process group. Never signal a caller's group.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
}
#[cfg(not(unix))]
fn kill_group(_: i32) {}
struct Cleanup(i32);
impl Drop for Cleanup {
    fn drop(&mut self) {
        kill_group(self.0);
    }
}
async fn frame(value: serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| invalid())?;
    bytes.push(b'\n');
    let mut output = tokio::io::stdout();
    output.write_all(&bytes).await?;
    output.flush().await
}
fn read_output(reader: impl AsyncRead + Unpin + Send + 'static, tx: mpsc::Sender<Vec<u8>>) {
    tokio::spawn(async move {
        let mut reader = reader;
        let mut bytes = vec![0; 16 * 1024];
        loop {
            match reader.read(&mut bytes).await {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    if tx.send(bytes[..size].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
}
pub async fn run() -> io::Result<()> {
    #[cfg(not(unix))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Command supervision requires macOS or Linux",
    ));
    #[cfg(unix)]
    {
        let group = std::process::id() as i32;
        if unsafe { libc::getpgrp() } != group {
            return Err(invalid());
        }
        let mut control = FramedRead::new(
            tokio::io::stdin(),
            LinesCodec::new_with_max_length(MAX_FRAME),
        );
        // Do not leave an uninitialized helper waiting forever on a broken parent.
        let first = tokio::time::timeout(Duration::from_secs(15), control.next())
            .await
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?
            .map_err(|_| invalid())?;
        let spec: Specification = serde_json::from_str(&first).map_err(|_| invalid())?;
        if spec.args.len() > 256
            || spec.args.iter().map(String::len).sum::<usize>() > 128 * 1024
            || !(100..=86400000).contains(&spec.timeout_ms)
        {
            return Err(invalid());
        }
        let mut lease_options = std::fs::OpenOptions::new();
        lease_options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false);
        use std::os::unix::fs::OpenOptionsExt;
        lease_options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let lease = lease_options.open(&spec.lease)?;
        lease.try_lock().map_err(|_| invalid())?;
        // Declared after the lease, so group cleanup runs before the descriptor
        // can release ownership on any ordinary return or error.
        let _cleanup = Cleanup(group);
        let epoch = Instant::now();
        let deadline = Arc::new(AtomicU64::new(15000));
        let watchdog = deadline.clone();
        let timeout_ms = spec.timeout_ms;
        // Independent of async output backpressure: a stalled parent cannot keep
        // commands alive merely by filling the supervisor's stdout pipe.
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let elapsed = epoch.elapsed().as_millis() as u64;
                if elapsed >= watchdog.load(Ordering::Acquire)
                    || elapsed >= timeout_ms.saturating_add(1000)
                {
                    kill_group(group);
                    break;
                }
            }
        });
        let stdin = Arc::new(Mutex::new(None::<tokio::process::ChildStdin>));
        let (launch_tx, launch_rx) = oneshot::channel();
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        let input = stdin.clone();
        tokio::spawn(async move {
            let mut launch = Some(launch_tx);
            while let Some(Ok(line)) = control.next().await {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    break;
                };
                match value["kind"].as_str() {
                    Some("lease") => {
                        deadline.store(
                            (epoch.elapsed().as_millis() as u64).saturating_add(15000),
                            Ordering::Release,
                        );
                    }
                    Some("launch") => {
                        let Some(sender) = launch.take() else {
                            break;
                        };
                        if sender.send(()).is_err() {
                            break;
                        }
                    }
                    Some("input") => {
                        let Some(encoded) = value["data"].as_str() else {
                            break;
                        };
                        let Ok(data) = STANDARD.decode(encoded) else {
                            break;
                        };
                        if data.len() > 64000 {
                            break;
                        }
                        let mut slot = input.lock().await;
                        let Some(writer) = slot.as_mut() else {
                            break;
                        };
                        if writer.write_all(&data).await.is_err() {
                            break;
                        }
                        if value["eof"] == true {
                            slot.take();
                        }
                    }
                    Some("stop") => break,
                    _ => break,
                }
            }
            let _ = stop_tx.send(()).await;
        });
        // No command can start until the parent acknowledges this held lease.
        // Recovery can safely probe the lease even if the parent dies mid-launch.
        frame(serde_json::json!({"kind":"ready"})).await?;
        tokio::select! {biased;_=stop_rx.recv()=>return Ok(()),result=launch_rx=>result.map_err(|_|invalid())?,}
        let mut command = Command::new(spec.file);
        command
            .args(spec.args)
            .current_dir(spec.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if let Some(env) = spec.env {
            command.env_clear().envs(env);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                frame(serde_json::json!({"kind":"exit","status":"failed","code":127})).await?;
                return Ok(());
            }
        };
        *stdin.lock().await = child.stdin.take();
        frame(serde_json::json!({"kind":"started"})).await?;
        let (tx, mut rx) = mpsc::channel(16);
        read_output(child.stdout.take().ok_or_else(invalid)?, tx.clone());
        read_output(child.stderr.take().ok_or_else(invalid)?, tx.clone());
        drop(tx);
        let timeout = tokio::time::sleep(Duration::from_millis(spec.timeout_ms));
        tokio::pin!(timeout);
        let (status, code) = loop {
            tokio::select! {
                biased;
                _=stop_rx.recv()=>break ("cancelled",None),
                _=&mut timeout=>break ("timed_out",None),
                exit=child.wait()=>break ("exited",exit.ok().and_then(|exit|exit.code())),
                Some(bytes)=rx.recv()=>frame(serde_json::json!({"kind":"output","data":STANDARD.encode(bytes)})).await?,
            }
        };
        if status == "exited" {
            // A background descendant can retain a pipe after its shell exits.
            // Drain the ordinary tail, then reclaim the complete owned group.
            // Bound descendants that continue writing after their shell exits.
            // The normal outstanding pipe/queue tail fits well below 64 frames.
            // Time out only waiting for more bytes, never an in-flight JSON write.
            for _ in 0..64 {
                let Ok(Some(bytes)) =
                    tokio::time::timeout(Duration::from_millis(250), rx.recv()).await
                else {
                    break;
                };
                frame(serde_json::json!({"kind":"output","data":STANDARD.encode(bytes)})).await?;
            }
        }
        frame(serde_json::json!({"kind":"exit","status":status,"code":code})).await?;
        Ok(())
    }
}
