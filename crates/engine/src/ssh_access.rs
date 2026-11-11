//! SSH into a cloud computer through its existing engine connection.
//!
//! The computer runs a private sshd on loopback only, started on demand with
//! the client's public key authorized. A client's ssh connects through a
//! ProxyCommand that opens [`open_tunnel`]: bytes from sshd stream back as
//! base64 chunks, and the client sends its bytes with [`write_tunnel`]. No
//! port is exposed; access rides the same authenticated path as the agent.
//!
//! Everything sshd needs lives under `~/.zuse-ssh`, including its host key, so
//! a computer that restarts without its memory (or only keeps home folders)
//! comes back with the same identity and restarts sshd on the next tunnel.
//! Computers with sudo run sshd as root with PAM; root-free computers run it
//! as the task user, which can then log in only as itself.
use anyhow::{Context as _, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt as _;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::Mutex;

/// The SSH server binary the computer image provides.
const SSHD: &str = "/usr/sbin/sshd";
/// Loopback port of the managed sshd.
pub const PORT: u16 = 2222;
/// Largest chunk one write or stream item carries.
const CHUNK: usize = 32 * 1024;

type Writer = Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>;
static TUNNELS: LazyLock<std::sync::Mutex<HashMap<String, Writer>>> =
    LazyLock::new(Default::default);

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/home/zuse".into())
}

fn ssh_dir() -> PathBuf {
    home().join(".zuse-ssh")
}

/// A single `ssh-ed25519`/`ecdsa`/`rsa` public key line, nothing else.
pub fn valid_public_key(key: &str) -> bool {
    let key = key.trim();
    let mut parts = key.split_whitespace();
    let (Some(kind), Some(body)) = (parts.next(), parts.next()) else {
        return false;
    };
    !key.contains(['\n', '\r'])
        && key.len() <= 8192
        && matches!(
            kind,
            "ssh-ed25519" | "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ssh-rsa"
        )
        && STANDARD.decode(body).is_ok()
}

fn host_key(dir: &Path) -> PathBuf {
    dir.join("host_ed25519_key")
}

/// How sshd runs: as root through sudo (with PAM), or as the task user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Sudo,
    User,
}

fn sshd_config(dir: &Path, user: &str, mode: Mode) -> String {
    let pam = if mode == Mode::Sudo { "yes" } else { "no" };
    format!(
        "Port {PORT}\n\
         ListenAddress 127.0.0.1\n\
         HostKey {host}\n\
         AuthorizedKeysFile {keys}\n\
         PasswordAuthentication no\n\
         KbdInteractiveAuthentication no\n\
         PubkeyAuthentication yes\n\
         PermitRootLogin no\n\
         AllowUsers {user}\n\
         StrictModes no\n\
         UsePAM {pam}\n\
         PrintMotd no\n\
         PidFile {pid}\n\
         Subsystem sftp /usr/lib/openssh/sftp-server\n",
        host = host_key(dir).display(),
        keys = dir.join("authorized_keys").display(),
        pid = dir.join("sshd.pid").display(),
    )
}

fn private_write(path: &Path, contents: &str) -> Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, path)?;
    Ok(())
}

async fn listening() -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", PORT))
        .await
        .is_ok()
}

async fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("could not run {program}"))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            args.iter()
                .find(|a| !a.starts_with('-'))
                .copied()
                .unwrap_or(program),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn sudo(args: &[&str]) -> Result<()> {
    let mut all = vec!["-n"];
    all.extend_from_slice(args);
    run("sudo", &all).await
}

async fn mode() -> Mode {
    if sudo(&["true"]).await.is_ok() {
        Mode::Sudo
    } else {
        Mode::User
    }
}

fn login_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "zuse".into())
}

/// The host's public key, as `<type> <base64>`, creating the pair once.
async fn ensure_host_key(dir: &Path) -> Result<String> {
    let key = host_key(dir);
    if !key.exists() {
        let path = key.to_string_lossy().into_owned();
        run(
            "ssh-keygen",
            &[
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "zuse-cloud-host",
                "-f",
                &path,
            ],
        )
        .await?;
    }
    let public = std::fs::read_to_string(key.with_extension("pub"))
        .context("The computer's SSH host key is unreadable.")?;
    Ok(public
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" "))
}

/// Stop the sshd recorded in the pid file and wait for its port to free.
async fn stop_previous(dir: &Path) {
    let Some(pid) = std::fs::read_to_string(dir.join("sshd.pid"))
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
    else {
        return;
    };
    let pid = pid.to_string();
    if run("kill", &[&pid]).await.is_err() {
        let _ = sudo(&["kill", &pid]).await;
    }
    for _ in 0..30 {
        if !listening().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Start the loopback sshd unless it already answers. Needs `prepare` to have
/// authorized a key once; after that it restarts sshd on its own.
async fn ensure_running(dir: &Path) -> Result<()> {
    let config = dir.join("sshd_config");
    if listening().await {
        let current = std::fs::read_to_string(&config).unwrap_or_default();
        if current.contains(&format!("HostKey {}\n", host_key(dir).display())) {
            return Ok(());
        }
        // An sshd started before host keys moved home serves another key.
        stop_previous(dir).await;
    }
    // Some computer images ship without an SSH server. Say so instead of
    // surfacing a launch error.
    ensure!(
        Path::new(SSHD).exists(),
        "SSH isn't available on this computer yet. Its image has no SSH server."
    );
    ensure_host_key(dir).await?;
    let mode = mode().await;
    private_write(&config, &sshd_config(dir, &login_user(), mode))?;
    let config = config.to_string_lossy().into_owned();
    match mode {
        Mode::Sudo => {
            sudo(&["mkdir", "-p", "/run/sshd"]).await?;
            sudo(&[SSHD, "-f", &config]).await?;
        }
        Mode::User => run(SSHD, &["-f", &config]).await?,
    }
    let started = tokio::time::Instant::now();
    while !listening().await {
        ensure!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "SSH did not start on the computer."
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Authorize `public_key` and make sure the loopback sshd runs. Returns the
/// user to log in as, the folder sessions should open, and the host key the
/// client pins for this computer.
pub async fn prepare(public_key: &str, workspace_root: Option<&Path>) -> Result<Value> {
    ensure!(
        valid_public_key(public_key),
        "That is not an SSH public key."
    );
    let dir = ssh_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let keys_path = dir.join("authorized_keys");
    let mut keys: Vec<String> = std::fs::read_to_string(&keys_path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let key = public_key.trim().to_owned();
    if !keys.contains(&key) {
        keys.push(key);
    }
    private_write(&keys_path, &(keys.join("\n") + "\n"))?;
    let host_key = ensure_host_key(&dir).await?;
    ensure_running(&dir).await?;
    let path = workspace_root
        .map(Path::to_path_buf)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| {
            let default = home().join("workspace");
            if default.is_dir() { default } else { home() }
        });
    Ok(json!({"user": login_user(), "path": path, "port": PORT, "hostKey": host_key}))
}

/// Connect to the loopback sshd. The stream's first item names the tunnel;
/// then each item carries `data` (base64) read from sshd, until it closes.
pub async fn open_tunnel() -> Result<futures::stream::BoxStream<'static, Value>> {
    let dir = ssh_dir();
    // sshd doesn't survive a restart without memory; bring it back.
    if !listening().await && dir.join("authorized_keys").exists() {
        ensure_running(&dir).await?;
    }
    ensure!(
        listening().await,
        "SSH is not running on the computer. Open via SSH again from Zuse."
    );
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", PORT)).await?;
    stream.set_nodelay(true).ok();
    let (read, write) = stream.into_split();
    let id = uuid::Uuid::new_v4().to_string();
    TUNNELS
        .lock()
        .unwrap()
        .insert(id.clone(), Arc::new(Mutex::new(write)));
    let first = futures::stream::once({
        let id = id.clone();
        async move { json!({"tunnel": id}) }
    });
    let data = futures::stream::unfold((read, id), |(mut read, id)| async move {
        let mut buffer = vec![0u8; CHUNK];
        match read.read(&mut buffer).await {
            Ok(0) | Err(_) => {
                TUNNELS.lock().unwrap().remove(&id);
                None
            }
            Ok(n) => Some((json!({"data": STANDARD.encode(&buffer[..n])}), (read, id))),
        }
    });
    Ok(first.chain(data).boxed())
}

/// Send bytes (base64) from the client into a tunnel, in order.
pub async fn write_tunnel(tunnel: &str, data: &str) -> Result<()> {
    let bytes = STANDARD.decode(data).context("Invalid tunnel data.")?;
    ensure!(bytes.len() <= CHUNK * 2, "Tunnel write too large.");
    let writer = TUNNELS
        .lock()
        .unwrap()
        .get(tunnel)
        .cloned()
        .context("That SSH connection has closed.")?;
    writer.lock().await.write_all(&bytes).await?;
    Ok(())
}

/// Close a tunnel's write side; sshd then ends the connection.
pub async fn close_tunnel(tunnel: &str) {
    let writer = TUNNELS.lock().unwrap().remove(tunnel);
    if let Some(writer) = writer {
        let _ = writer.lock().await.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_line_public_keys_are_accepted() {
        assert!(valid_public_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl zuse"
        ));
        assert!(!valid_public_key("ssh-ed25519"));
        assert!(!valid_public_key("not-a-key AAAA"));
        assert!(!valid_public_key(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\ncommand=\"rm -rf /\" ssh-ed25519 AAAA"
        ));
    }

    #[test]
    fn sshd_listens_on_loopback_with_keys_only() {
        let config = sshd_config(Path::new("/home/zuse/.zuse-ssh"), "zuse", Mode::Sudo);
        assert!(config.contains("ListenAddress 127.0.0.1"));
        assert!(config.contains("HostKey /home/zuse/.zuse-ssh/host_ed25519_key\n"));
        assert!(config.contains("UsePAM yes"));
        // Without root, PAM can't run; the host key still lives in home.
        let user = sshd_config(Path::new("/home/prized/.zuse-ssh"), "prized", Mode::User);
        assert!(user.contains("UsePAM no"));
        assert!(user.contains("AllowUsers prized"));
        assert!(config.contains("PasswordAuthentication no"));
        assert!(config.contains("AllowUsers zuse"));
        assert!(config.contains("AuthorizedKeysFile /home/zuse/.zuse-ssh/authorized_keys"));
    }

    #[tokio::test]
    async fn a_tunnel_pipes_bytes_both_ways() {
        // A stand-in for sshd on the managed port, when it is free here.
        let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await else {
            return;
        };
        // Probes connect and drop; echo for the first client that writes.
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 5];
                if socket.read_exact(&mut buffer).await.is_ok() {
                    socket.write_all(&buffer).await.unwrap();
                    return;
                }
            }
        });
        let mut stream = open_tunnel().await.unwrap();
        let first = stream.next().await.unwrap();
        let tunnel = first["tunnel"].as_str().unwrap().to_owned();
        write_tunnel(&tunnel, &STANDARD.encode(b"hello"))
            .await
            .unwrap();
        let echoed = stream.next().await.unwrap();
        assert_eq!(
            STANDARD.decode(echoed["data"].as_str().unwrap()).unwrap(),
            b"hello"
        );
        close_tunnel(&tunnel).await;
        server.await.unwrap();
    }
}
