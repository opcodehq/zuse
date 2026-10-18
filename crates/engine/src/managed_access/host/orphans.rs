//! Recover only connectors with this profile's exact binary and token-file argv.
use super::super::{Error, Result};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;
const MAX_PROCESS_BYTES: u64 = 8 * 1024 * 1024;
fn expected(root: &Path, token: &Path) -> String {
    format!(
        "{} tunnel --no-autoupdate run --token-file {}",
        root.join("runtime/cloudflared")
            .join(super::install::VERSION)
            .join("cloudflared")
            .display(),
        token.display()
    )
}
async fn processes(pid: Option<u32>) -> Result<String> {
    let mut command = tokio::process::Command::new("/bin/ps");
    command.arg("-ww");
    if let Some(pid) = pid {
        command.args(["-p", &pid.to_string(), "-o", "pid=,command="]);
    } else {
        command.args(["-axo", "pid=,command="]);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::Unavailable)?;
    let mut bytes = Vec::new();
    let mut stdout = child
        .stdout
        .take()
        .ok_or(Error::Unavailable)?
        .take(MAX_PROCESS_BYTES + 1);
    tokio::time::timeout(Duration::from_secs(3), stdout.read_to_end(&mut bytes))
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
    if bytes.len() as u64 > MAX_PROCESS_BYTES {
        return Err(Error::Invalid);
    }
    let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
    // ps returns 1 for a PID that exited between the two observations.
    if !status.success() && !(pid.is_some() && bytes.is_empty()) {
        return Err(Error::Unavailable);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
fn owned_pid(line: &str, expected: &str) -> Option<u32> {
    let line = line.trim_start();
    let split = line.find(char::is_whitespace)?;
    let pid = line[..split].parse::<u32>().ok()?;
    (pid > 1 && pid != std::process::id() && line[split..].trim() == expected).then_some(pid)
}
async fn is_owned(pid: u32, expected: &str) -> Result<bool> {
    Ok(processes(Some(pid))
        .await?
        .lines()
        .any(|line| owned_pid(line, expected) == Some(pid)))
}
pub(super) async fn cleanup(root: &Path) -> Result<()> {
    let directory = root.join("managed-access");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(Error::Storage),
    };
    let mut tokens: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| Error::Storage)?;
        if entry.file_name().to_str().is_some_and(|s| {
            s.strip_prefix("connector-").is_some_and(|suffix| {
                suffix.len() == 12 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
            })
        }) && entry.file_type().map_err(|_| Error::Storage)?.is_file()
        {
            tokens.push(entry.path());
            if tokens.len() > 128 {
                return Err(Error::Invalid);
            }
        }
    }
    if tokens.is_empty() {
        return Ok(());
    }
    let list = processes(None).await?;
    for token in tokens {
        let command = expected(root, &token);
        for pid in list.lines().filter_map(|line| owned_pid(line, &command)) {
            if !is_owned(pid, &command).await? {
                continue;
            }
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            for _ in 0..20 {
                if !is_owned(pid, &command).await? {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if is_owned(pid, &command).await? {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
                for _ in 0..10 {
                    if !is_owned(pid, &command).await? {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                if is_owned(pid, &command).await? {
                    return Err(Error::Unavailable);
                }
            }
        }
        match std::fs::remove_file(token) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Storage),
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_identity_requires_the_complete_owned_command() {
        let root = Path::new("/tmp/profile with spaces");
        let token = root.join("managed-access/connector-private");
        let command = expected(root, &token);
        assert_eq!(
            owned_pid(&format!("  45678 {command}"), &command),
            Some(45678)
        );
        assert_eq!(
            owned_pid(&format!("45678 /bin/echo {command}"), &command),
            None
        );
        assert_eq!(owned_pid(&format!("45678 {command} extra"), &command), None);
        assert_eq!(
            owned_pid(&format!("45678 {command}-another-profile"), &command),
            None
        );
    }
    #[tokio::test]
    async fn stale_token_files_do_not_kill_unrelated_processes() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("managed-access");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("connector-ABCDEF123456"), "fixture-secret").unwrap();
        std::fs::write(dir.join("identity.json"), "preserved").unwrap();
        cleanup(root.path()).await.unwrap();
        assert!(!dir.join("connector-ABCDEF123456").exists());
        assert!(dir.join("identity.json").exists());
    }
}
