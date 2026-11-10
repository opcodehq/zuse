//! SSH into a cloud computer through its existing engine connection.
//!
//! The computer runs a private sshd on loopback only, started on demand with
//! the client's public key authorized. A client's ssh connects through a
//! ProxyCommand that opens [`open_tunnel`]: bytes from sshd stream back as
//! base64 chunks, and the client sends its bytes with [`write_tunnel`]. No
//! port is exposed; access rides the same authenticated path as the agent.
use anyhow::{Context as _, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt as _;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::Mutex;

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

fn sshd_config(dir: &Path, user: &str) -> String {
    format!(
        "Port {PORT}\n\
         ListenAddress 127.0.0.1\n\
         HostKey /etc/ssh/ssh_host_ed25519_key\n\
         HostKey /etc/ssh/ssh_host_rsa_key\n\
         AuthorizedKeysFile {keys}\n\
         PasswordAuthentication no\n\
         KbdInteractiveAuthentication no\n\
         PubkeyAuthentication yes\n\
         PermitRootLogin no\n\
         AllowUsers {user}\n\
         StrictModes no\n\
         UsePAM yes\n\
         PrintMotd no\n\
         PidFile {pid}\n\
         Subsystem sftp /usr/lib/openssh/sftp-server\n",
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

async fn sudo(args: &[&str]) -> Result<()> {
    let status = tokio::process::Command::new("sudo")
        .arg("-n")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .with_context(|| format!("could not run {}", args.join(" ")))?;
    if !status.status.success() {
        bail!(
            "{} failed: {}",
            args.first().copied().unwrap_or("command"),
            String::from_utf8_lossy(&status.stderr).trim()
        );
    }
    Ok(())
}

/// Authorize `public_key` and make sure the loopback sshd runs. Returns the
/// user to log in as and the folder sessions should open.
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
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "zuse".into());
    let config = dir.join("sshd_config");
    private_write(&config, &sshd_config(&dir, &user))?;
    if !listening().await {
        if !Path::new("/etc/ssh/ssh_host_ed25519_key").exists() {
            sudo(&["ssh-keygen", "-A"]).await?;
        }
        sudo(&["mkdir", "-p", "/run/sshd"]).await?;
        let config = config.to_string_lossy().into_owned();
        sudo(&["/usr/sbin/sshd", "-f", &config]).await?;
        let started = tokio::time::Instant::now();
        while !listening().await {
            ensure!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "SSH did not start on the computer."
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    let path = workspace_root
        .map(Path::to_path_buf)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| {
            let default = home().join("workspace");
            if default.is_dir() { default } else { home() }
        });
    Ok(json!({"user": user, "path": path, "port": PORT}))
}

/// Connect to the loopback sshd. The stream's first item names the tunnel;
/// then each item carries `data` (base64) read from sshd, until it closes.
pub async fn open_tunnel() -> Result<futures::stream::BoxStream<'static, Value>> {
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
        let config = sshd_config(Path::new("/home/zuse/.zuse-ssh"), "zuse");
        assert!(config.contains("ListenAddress 127.0.0.1"));
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
