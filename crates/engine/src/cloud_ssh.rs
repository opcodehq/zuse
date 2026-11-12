//! "Open via SSH": a key for this device, a `zuse-<workspace>` host in a
//! managed ssh config, and the computer told to accept that key. ssh then
//! reaches it through `zuse ssh-proxy`, over the app's own cloud connection.
use std::path::{Path, PathBuf};

/// What a prepared host needs to be opened or copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub host: String,
    pub path: String,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| ".".into())
}
fn root() -> PathBuf {
    home().join(".zuse/ssh")
}
fn key_path() -> PathBuf {
    root().join("id_ed25519")
}

pub fn host_alias(workspace: &str) -> String {
    format!("zuse-{workspace}")
}

/// The one-line known-hosts file for a workspace's computer.
fn known_hosts(workspace: &str) -> PathBuf {
    root().join("hosts").join(format!("{workspace}.known"))
}

/// Trust exactly the host key the computer reported over the app's own
/// authenticated connection. Rewritten on every prepare, so a computer that
/// is replaced keeps working without a "host key changed" warning. Blocking.
pub fn pin_host_key(workspace: &str, host_key: &str) -> Result<(), String> {
    let mut parts = host_key.split_whitespace();
    let (Some(kind), Some(body), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("The computer reported an invalid SSH host key.".into());
    };
    if !kind.starts_with("ssh-") && !kind.starts_with("ecdsa-") {
        return Err("The computer reported an invalid SSH host key.".into());
    }
    let hosts = root().join("hosts");
    private_dir(&hosts).map_err(|e| e.to_string())?;
    private_write(
        &known_hosts(workspace),
        &format!("{} {kind} {body}\n", host_alias(workspace)),
    )
    .map_err(|e| e.to_string())
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, path)
}

/// This device's public key, creating the pair on first use. Blocking.
pub fn ensure_key() -> Result<String, String> {
    private_dir(&root()).map_err(|e| e.to_string())?;
    let key = key_path();
    if !key.exists() {
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "zuse-cloud", "-f"])
            .arg(&key)
            .stdin(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("Couldn't run ssh-keygen: {e}"))?;
        if !status.success() {
            return Err("ssh-keygen couldn't create a key.".into());
        }
    }
    std::fs::read_to_string(key.with_extension("pub"))
        .map(|key| key.trim().to_owned())
        .map_err(|e| e.to_string())
}

/// The managed host entry for one workspace.
pub fn host_entry(workspace: &str, user: &str, proxy: &str) -> String {
    let host = host_alias(workspace);
    format!(
        "Host {host}\n\
         \x20 HostName {host}\n\
         \x20 User {user}\n\
         \x20 IdentityFile {key}\n\
         \x20 IdentitiesOnly yes\n\
         \x20 ProxyCommand {proxy} %h\n\
         \x20 StrictHostKeyChecking accept-new\n\
         \x20 UserKnownHostsFile {known}\n\
         \x20 ServerAliveInterval 30\n",
        key = key_path().display(),
        known = known_hosts(workspace).display(),
    )
}

/// Write the host entry and make `~/.ssh/config` include the managed config,
/// so `ssh zuse-<workspace>` works from any terminal or editor. Blocking.
pub fn install_host(workspace: &str, user: &str, ipc_port: u16) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let proxy = format!(
        "{} ssh-proxy --ipc-port {ipc_port}",
        quote(&exe.to_string_lossy())
    );
    let hosts = root().join("hosts");
    private_dir(&hosts).map_err(|e| e.to_string())?;
    private_write(
        &hosts.join(format!("{workspace}.conf")),
        &host_entry(workspace, user, &proxy),
    )
    .map_err(|e| e.to_string())?;
    let managed = root().join("config");
    private_write(
        &managed,
        &format!(
            "# Zuse cloud computers. Managed by Zuse; edits are replaced.\nInclude {}/*.conf\n",
            hosts.display()
        ),
    )
    .map_err(|e| e.to_string())?;
    let ssh = home().join(".ssh");
    private_dir(&ssh).map_err(|e| e.to_string())?;
    let user_config = ssh.join("config");
    let existing = std::fs::read_to_string(&user_config).unwrap_or_default();
    let include = format!("Include {}", managed.display());
    if !existing.lines().any(|line| line.trim() == include) {
        // Top of the file, before any Host block, so it applies everywhere.
        let updated = format!("{include}\n\n{existing}");
        private_write(&user_config, &updated).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The command a terminal runs: a login shell in the repository.
pub fn terminal_command(prepared: &Prepared) -> String {
    format!(
        "ssh -t {} {}",
        prepared.host,
        quote(&format!(
            "cd {} && exec \"$SHELL\" -l",
            quote(&prepared.path)
        ))
    )
}

/// Open the system terminal running `command`. Blocking.
pub fn open_terminal(command: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
            command.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let status = std::process::Command::new("osascript")
            .args(["-e", &script])
            .status()
            .map_err(|e| e.to_string())?;
        return if status.success() {
            Ok(())
        } else {
            Err("Terminal didn't open.".into())
        };
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::process::Command::new("x-terminal-emulator")
            .args(["-e", "sh", "-c", command])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't open a terminal: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_entries_route_through_the_proxy_with_a_private_known_hosts() {
        let entry = host_entry(
            "workspace_1",
            "zuse",
            "'/Applications/Zuse.app/zuse' ssh-proxy --ipc-port 27654",
        );
        assert!(entry.starts_with("Host zuse-workspace_1\n"));
        assert!(entry.contains("  User zuse\n"));
        assert!(entry.contains(
            "  ProxyCommand '/Applications/Zuse.app/zuse' ssh-proxy --ipc-port 27654 %h\n"
        ));
        assert!(entry.contains("IdentitiesOnly yes"));
        assert!(entry.contains("hosts/workspace_1.known\n"));
    }

    #[test]
    fn pinned_host_keys_reject_anything_but_one_key() {
        assert!(pin_host_key("w", "ssh-ed25519 AAAA\nzuse-other ssh-ed25519 BBBB").is_err());
        assert!(pin_host_key("w", "not-a-key AAAA").is_err());
        assert!(pin_host_key("w", "ssh-ed25519").is_err());
    }

    #[test]
    fn terminal_commands_open_the_repository_with_quoting_intact() {
        let command = terminal_command(&Prepared {
            host: "zuse-w".into(),
            path: "/home/zuse/my repo".into(),
        });
        assert_eq!(
            command,
            r#"ssh -t zuse-w 'cd '\''/home/zuse/my repo'\'' && exec "$SHELL" -l'"#
        );
    }
}
