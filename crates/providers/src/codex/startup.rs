//! Serialize managed Codex profile initialization across all native processes.
//! There is deliberately no "initialized" marker: every native initialize
//! handshake owns this lock until its response, including after a cold restore.
use crate::{HarnessError, process::Command};
use std::{fs::File, path::PathBuf};
use tokio_util::sync::CancellationToken;

pub(super) struct Guard {
    _file: File,
}
fn failure(code: &'static str) -> HarnessError {
    HarnessError::Protocol(code.into())
}

#[cfg(target_os = "linux")]
fn environment(command: &Command, name: &str) -> Option<std::ffi::OsString> {
    if let Some((_, value)) = command.as_std().get_envs().find(|(key, _)| *key == name) {
        return value.map(std::ffi::OsStr::to_os_string);
    }
    std::env::var_os(name)
}
#[cfg(target_os = "linux")]
fn profile(command: &Command) -> Result<PathBuf, HarnessError> {
    let path =
        if let Some(home) = environment(command, "CODEX_HOME").filter(|value| !value.is_empty()) {
            PathBuf::from(home)
        } else {
            PathBuf::from(
                environment(command, "HOME")
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| failure("codex-managed-profile-home-required"))?,
            )
            .join(".codex")
        };
    if path.is_absolute() {
        Ok(path)
    } else {
        let cwd = command
            .as_std()
            .get_current_dir()
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir().map_err(HarnessError::Io)?);
        Ok(cwd.join(path))
    }
}
#[cfg(target_os = "linux")]
fn open(path: PathBuf) -> Result<File, HarnessError> {
    use std::{
        fs::{self, OpenOptions},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    };
    let invalid = || failure("codex-managed-profile-unsafe");
    let mut ancestor = PathBuf::new();
    for component in path.components() {
        ancestor.push(component);
        match fs::symlink_metadata(&ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(invalid()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(invalid()),
        }
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&path)
        .map_err(|_| invalid())?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| invalid())?;
    // Only a managed task's own profile is eligible. Tightening read/execute
    // permissions is safe; an unexpectedly writable/shared profile is not.
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err(invalid());
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|_| invalid())?;
    let canonical = fs::canonicalize(&path).map_err(|_| invalid())?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(canonical.join(".zuse-native-initialize.lock"))
        .map_err(|_| invalid())?;
    let metadata = lock.metadata().map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() != 0
    {
        return Err(invalid());
    }
    Ok(lock)
}

pub(super) async fn acquire(
    command: &Command,
    cancel: Option<&CancellationToken>,
) -> Result<Guard, HarnessError> {
    #[cfg(target_os = "linux")]
    {
        let path = profile(command)?;
        let file = tokio::task::spawn_blocking(move || open(path))
            .await
            .map_err(|_| failure("codex-managed-profile-unavailable"))??;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(failure("codex-managed-initialize-cancelled"));
            }
            match file.try_lock() {
                Ok(()) => return Ok(Guard { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(_) => return Err(failure("codex-managed-initialize-lock-failed")),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(failure("codex-managed-initialize-busy"));
            }
            match cancel {
                Some(cancel) => {
                    tokio::select! {_=cancel.cancelled()=>return Err(failure("codex-managed-initialize-cancelled")),_=tokio::time::sleep(std::time::Duration::from_millis(10))=>{}}
                }
                None => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (command, cancel);
        Err(failure("codex-managed-profile-platform-unsupported"))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn command(home: &std::path::Path) -> Command {
        let mut command = Command::new("/bin/true");
        command.env("HOME", home).env_remove("CODEX_HOME");
        command
    }
    #[test]
    fn managed_profile_creation_is_private_under_group_writable_umask() {
        const CHILD: &str = "ZUSE_CODEX_STARTUP_UMASK_FIXTURE";
        if let Some(path) = std::env::var_os(CHILD) {
            let path = PathBuf::from(path);
            // The parent starts THIS ONE test in a separate process. No other
            // test's filesystem writes observe this intentionally weak umask.
            unsafe {
                libc::umask(0o002);
            }
            drop(open(path.join("new/.codex")).unwrap());
            assert_eq!(
                std::fs::metadata(path.join("new/.codex"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(path.join("new/.codex/.zuse-native-initialize.lock"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            let shared = path.join("existing");
            std::fs::create_dir(&shared).unwrap();
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o770)).unwrap();
            assert!(open(shared.clone()).is_err());
            assert_eq!(
                std::fs::metadata(shared).unwrap().permissions().mode() & 0o777,
                0o770
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let result=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","codex::startup::tests::managed_profile_creation_is_private_under_group_writable_umask","--nocapture"])
            .env(CHILD,temp.path()).output().unwrap();
        assert!(
            result.status.success(),
            "isolated umask regression failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("1 passed"),
            "child test must execute"
        );
    }
    #[tokio::test]
    async fn managed_profile_guard_is_private_os_locked_and_failure_releases_it() {
        let temp = tempfile::tempdir().unwrap();
        let command = command(temp.path());
        let guard = acquire(&command, None).await.unwrap();
        let profile = temp.path().join(".codex");
        assert_eq!(
            std::fs::metadata(&profile).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let lock = profile.join(".zuse-native-initialize.lock");
        assert_eq!(
            std::fs::metadata(&lock).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let external = |blocked: bool| {
            std::process::Command::new("python3").arg("-c").arg("import fcntl,sys; f=open(sys.argv[1],'r+');\ntry: fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB); blocked=False\nexcept BlockingIOError: blocked=True\nsys.exit(0 if blocked==(sys.argv[2]=='yes') else 1)").arg(&lock).arg(if blocked{"yes"}else{"no"}).status().unwrap()
        };
        assert!(external(true).success());
        // A failed first initialize drops this same guard, without a marker.
        drop(guard);
        assert!(external(false).success());
        drop(acquire(&command, None).await.unwrap());
    }
    #[tokio::test]
    async fn managed_profile_wait_cancellation_and_symlinks_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let command = command(temp.path());
        let held = acquire(&command, None).await.unwrap();
        let cancel = CancellationToken::new();
        let fire = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            fire.cancel();
        });
        assert!(
            matches!(acquire(&command,Some(&cancel)).await,Err(HarnessError::Protocol(ref value)) if value=="codex-managed-initialize-cancelled")
        );
        drop(held);
        drop(acquire(&command, None).await.unwrap());
        let lock = temp.path().join(".codex/.zuse-native-initialize.lock");
        std::fs::remove_file(&lock).unwrap();
        symlink(temp.path().join("outside"), &lock).unwrap();
        assert!(acquire(&command, None).await.is_err());
        assert!(!temp.path().join("outside").exists());
    }
}
