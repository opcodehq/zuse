//! Task-local gh launcher. The broker supplies one repository's user token only
//! to the real CLI child, never to arguments, gitconfig or a global environment.
use super::*;
use std::ffi::OsString;
pub const MANAGED_GH: &str = "/usr/bin/gh";
fn supported_args(args: &[OsString]) -> bool {
    if args.first().is_some_and(|a| a == "auth") && args.get(1).is_none_or(|a| a != "status") {
        return false;
    }
    for (index, arg) in args.iter().enumerate() {
        let Some(option) = arg.to_str() else {
            return false;
        };
        // Cobra accepts joined/clustered shorthand options as well as -h HOST.
        // Auth status's -t/--show-token would otherwise export the broker token.
        let short = option.strip_prefix('-').filter(|v| !v.starts_with('-'));
        if short.is_some_and(|v| v.len() > 1 && v.contains('h'))
            || (args.first().is_some_and(|v| v == "auth")
                && (option.starts_with("--show-token") || short.is_some_and(|v| v.contains('t'))))
        {
            return false;
        }
        if let Some(value) = arg
            .to_str()
            .filter(|v| v.starts_with("http://") || v.starts_with("https://"))
        {
            let Ok(url) = reqwest::Url::parse(value) else {
                return false;
            };
            if url.scheme() != "https"
                || !matches!(url.host_str(), Some("github.com" | "api.github.com"))
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
            {
                return false;
            }
        }

        if arg == "-h"
            && args
                .get(index + 1)
                .is_none_or(|value| value != "github.com")
        {
            return false;
        }
        if arg == "--hostname" && args.get(index + 1).is_none_or(|v| v != "github.com") {
            return false;
        }
        if arg
            .to_str()
            .is_some_and(|s| s.starts_with("--hostname=") && s != "--hostname=github.com")
        {
            return false;
        }
    }
    true
}
pub fn managed_cli_version() -> Result<String> {
    let path = Path::new(MANAGED_GH);
    ensure!(
        std::fs::symlink_metadata(path)?.file_type().is_file(),
        "github_cli_managed_binary_missing"
    );
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)?;
    ensure!(
        metadata.uid() == 0 && metadata.permissions().mode() & 0o022 == 0,
        "github_cli_managed_binary_untrusted"
    );
    let output = std::process::Command::new(path)
        .arg("--version")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .output()?;
    ensure!(
        output.status.success() && output.stdout.len() <= 1024,
        "github_cli_version_invalid"
    );
    let line = std::str::from_utf8(&output.stdout)?
        .lines()
        .next()
        .unwrap_or_default();
    ensure!(
        line.starts_with("gh version 2.") && !line.chars().any(char::is_control),
        "github_cli_version_unsupported"
    );
    Ok(line.into())
}
const MANAGED_WRAPPER: &str = "/opt/zuse/bin/gh";
const MANAGED_WRAPPER_BYTES: &[u8] =
    b"#!/bin/sh\nexec /usr/local/bin/zuse-cloud-runtime github-from-profile \"$@\"\n";
const MANAGED_PROFILE: &str = "/etc/profile.d/zz-zuse-managed-bin.sh";
const MANAGED_PROFILE_BYTES: &[u8] =
    br#"# Zuse managed commands; retain an existing task-specific PATH prefix.
case ":${PATH:-}:" in
  *":/opt/zuse/bin:"*) ;;
  *) PATH="/opt/zuse/bin:${PATH:-/usr/bin:/bin}" ;;
esac
export PATH
"#;
fn controlled_directory(path: &Path, uid: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o022 == 0,
        "github_cli_managed_ancestor_invalid"
    );
    Ok(())
}
fn managed_profile(path: &Path, uid: u32) -> Result<()> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == uid
            && metadata.nlink() == 1
            && metadata.mode() & 0o777 == 0o644
            && metadata.len() <= 4096,
        "github_cli_managed_profile_invalid"
    );
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    ensure!(
        bytes == MANAGED_PROFILE_BYTES,
        "github_cli_managed_profile_invalid"
    );
    Ok(())
}
fn wrapper_sha(path: &Path, expected_uid: u32) -> Result<String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == expected_uid
            && metadata.nlink() == 1
            && metadata.mode() & 0o022 == 0
            && metadata.mode() & 0o111 != 0
            && metadata.len() <= 4096,
        "github_cli_managed_wrapper_invalid"
    );
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    ensure!(
        bytes == MANAGED_WRAPPER_BYTES,
        "github_cli_managed_wrapper_invalid"
    );
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}
fn login_shell_proof(stdout: &[u8], success: bool, version: &str) -> bool {
    if !success || stdout.len() > 4096 {
        return false;
    }
    let Ok(text) = std::str::from_utf8(stdout) else {
        return false;
    };
    let mut lines = text.lines();
    lines.next() == Some(MANAGED_WRAPPER) && lines.next() == Some(version)
}
pub fn managed_cli_receipt() -> Result<crate::image::GithubCliReceipt> {
    let version = managed_cli_version()?;
    ensure!(
        std::fs::metadata(MANAGED_GH)?.len() <= 128 * 1024 * 1024,
        "github_cli_managed_binary_invalid"
    );
    let bytes = std::fs::read(MANAGED_GH)?;
    for path in [
        "/opt",
        "/opt/zuse",
        "/opt/zuse/bin",
        "/etc",
        "/etc/profile.d",
    ] {
        controlled_directory(Path::new(path), 0)?;
    }
    managed_profile(Path::new(MANAGED_PROFILE), 0)?;
    let wrapper_sha256 = wrapper_sha(Path::new(MANAGED_WRAPPER), 0)?;
    // Image-only probe: no task profile or credential is provisioned. GNU
    // timeout bounds profile startup; only command resolution/version run.
    let shell = std::process::Command::new("/usr/bin/timeout")
        .args([
            "--kill-after=1s",
            "5s",
            "/bin/bash",
            "-lc",
            "command -v gh && gh --version",
        ])
        .env_clear()
        .env("HOME", "/home/zuse")
        .env("USER", "zuse")
        .env("LOGNAME", "zuse")
        .env(
            "PATH",
            "/opt/zuse/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .output()?;
    ensure!(
        login_shell_proof(&shell.stdout, shell.status.success(), &version),
        "github_cli_managed_login_path_invalid"
    );
    Ok(crate::image::GithubCliReceipt {
        version,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        wrapper_sha256: Some(wrapper_sha256),
        login_shell_resolved_managed: Some(true),
    })
}
impl GithubAuth {
    pub fn cli_directory(&self) -> PathBuf {
        self.directory.join("bin")
    }
    pub fn install_cli(&self, executable: &Path) -> Result<PathBuf> {
        managed_cli_version()?;
        let bin = self.directory.join("bin");
        std::fs::DirBuilder::new().mode(0o700).create(&bin)?;
        let path = bin.join("gh");
        // Everything interpolated here is non-secret and shell-quoted. The
        // managed executable implements validation, authority and child auth.
        let script = format!(
            "#!/bin/sh\nexec {} github-cli --socket {} --repository {} -- \"$@\"\n",
            shell_quote(executable),
            shell_quote(&self.socket),
            shell_quote(Path::new(&self.broker.repository))
        );
        crate::identity::atomic_write(&path, script.as_bytes())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        crate::identity::atomic_write(
            Path::new(PROFILE),
            &serde_json::to_vec(
                &json!({"version":1,"workspaceId":self.broker.recipient.workspace,"launcher":path}),
            )?,
        )?;
        Ok(path)
    }
}
pub fn run_cli(socket: &Path, repository: &str, args: &[OsString]) -> Result<i32> {
    ensure!(supported_args(args), "github_cli_managed_auth_required");
    ensure!(valid_repository(repository), "github_repository_invalid");
    managed_cli_version()?;
    let input = format!("protocol=https\nhost=github.com\npath={repository}.git\n\n");
    let response = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(helper(socket, "get", input.as_bytes()))?;
    let secret = token(&response)?;
    exec_child(
        Path::new(MANAGED_GH),
        args,
        &secret,
        &socket.with_extension("gh-config"),
    )
}
fn token(response: &[u8]) -> Result<Zeroizing<String>> {
    let value = std::str::from_utf8(response)?;
    let mut tokens = value
        .lines()
        .filter_map(|line| line.strip_prefix("password="));
    let token = tokens
        .next()
        .filter(|v| !v.is_empty() && v.len() <= 32768 && !v.chars().any(char::is_control))
        .ok_or_else(|| anyhow::anyhow!("github_credential_unavailable"))?;
    ensure!(tokens.next().is_none(), "github_credential_unavailable");
    Ok(Zeroizing::new(token.into()))
}
fn command(
    binary: &Path,
    args: &[OsString],
    secret: &str,
    config: &Path,
) -> Result<std::process::Command> {
    ensure!(
        binary.is_absolute() && binary.is_file(),
        "github_cli_managed_binary_missing"
    );
    if !config.exists() {
        std::fs::DirBuilder::new().mode(0o700).create(config)?;
    }
    let metadata = std::fs::symlink_metadata(config)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.permissions().mode() & 0o077 == 0,
        "github_cli_config_invalid"
    );
    let mut command = std::process::Command::new(binary);
    command
        .args(args)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .env_remove("GH_DEBUG")
        .env("GH_TOKEN", secret)
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_CONFIG_DIR", config);
    // Do not format/log command: its private OS child environment contains the
    // ephemeral token. ProcessRequest in the parent contains only this launcher.
    Ok(command)
}
fn exec_child(binary: &Path, args: &[OsString], secret: &str, config: &Path) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    // Replace this launcher so cancellation targets the actual CLI process,
    // rather than leaving an orphan that might finish a write after timeout.
    Err(command(binary, args, secret, config)?.exec().into())
}
#[cfg(test)]
fn child(binary: &Path, args: &[OsString], secret: &str, config: &Path) -> Result<i32> {
    Ok(command(binary, args, secret, config)?
        .status()?
        .code()
        .unwrap_or(1))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_receipt_requires_exact_private_wrapper_and_actual_login_resolution() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gh");
        std::fs::write(&path, MANAGED_WRAPPER_BYTES).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let uid = unsafe { libc::geteuid() };
        assert_eq!(wrapper_sha(&path, uid).unwrap().len(), 64);
        assert!(wrapper_sha(&path, u32::MAX).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(wrapper_sha(&path, uid).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&path, b"#!/bin/sh\nexec gh \"$@\"\n").unwrap();
        assert!(wrapper_sha(&path, uid).is_err());
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(wrapper_sha(&alias, uid).is_err());
        assert!(login_shell_proof(
            b"/opt/zuse/bin/gh\ngh version 2.79.0\nrelease info\n",
            true,
            "gh version 2.79.0"
        ));
        assert!(!login_shell_proof(
            b"/usr/bin/gh\ngh version 2.79.0\n",
            true,
            "gh version 2.79.0"
        ));
        assert!(!login_shell_proof(
            b"/opt/zuse/bin/gh\ngh version 2.78.0\n",
            true,
            "gh version 2.79.0"
        ));
        assert!(!login_shell_proof(
            b"/opt/zuse/bin/gh\ngh version 2.79.0\n",
            false,
            "gh version 2.79.0"
        ));
    }
    #[test]
    fn managed_directory_and_profile_reject_writable_or_redirected_entries() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("bin");
        std::fs::create_dir(&directory).unwrap();
        let uid = unsafe { libc::geteuid() };
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
        controlled_directory(&directory, uid).unwrap();
        assert!(controlled_directory(&directory, u32::MAX).is_err());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(controlled_directory(&directory, uid).is_err());
        let redirected = temporary.path().join("redirect");
        std::os::unix::fs::symlink(&directory, &redirected).unwrap();
        assert!(controlled_directory(&redirected, uid).is_err());
        let profile = temporary.path().join("profile");
        std::fs::write(&profile, MANAGED_PROFILE_BYTES).unwrap();
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o644)).unwrap();
        managed_profile(&profile, uid).unwrap();
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o664)).unwrap();
        assert!(managed_profile(&profile, uid).is_err());
        std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&profile, b"export PATH=/wrong").unwrap();
        assert!(managed_profile(&profile, uid).is_err());
        assert!(!login_shell_proof(
            b"/usr/local/bin/gh\ngh version 2.79.0\n",
            true,
            "gh version 2.79.0"
        ));
    }
    #[test]
    fn gh_auth_never_exports_tokens_or_overrides_the_pinned_host() {
        for args in [
            vec!["auth", "token"],
            vec!["auth", "login"],
            vec!["api", "--hostname=evil.test"],
            vec!["auth", "status", "-h", "evil.test"],
            vec!["auth", "status", "-hevil.test"],
            vec!["auth", "status", "-h=evil.test"],
            vec!["auth", "status", "-ah=evil.test"],
            vec!["auth", "status", "--show-token"],
            vec!["auth", "status", "--show-token=true"],
            vec!["auth", "status", "-t"],
            vec!["auth", "status", "-at"],
            vec!["api", "https://evil.test/resource"],
        ] {
            assert!(!supported_args(
                &args.into_iter().map(OsString::from).collect::<Vec<_>>()
            ));
        }
        assert!(supported_args(&[
            OsString::from("auth"),
            OsString::from("status")
        ]));
        assert!(supported_args(&[
            OsString::from("pr"),
            OsString::from("create"),
            OsString::from("--draft")
        ]));
        assert!(token(b"password=first\npassword=second\n").is_err());
    }
    #[test]
    fn real_child_gets_scoped_token_only_in_its_ephemeral_environment() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("gh");
        let proof = dir.path().join("proof");
        std::fs::write(&binary,format!("#!/bin/sh\n[ \"$GH_TOKEN\" = scoped-test-token ] || exit 41\n[ \"$GH_HOST\" = github.com ] || exit 42\n[ -z \"${{GITHUB_TOKEN+x}}\" ] || exit 43\nprintf '%s' \"$*\" > '{}'\n",proof.display())).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let args = vec![
            OsString::from("pr"),
            OsString::from("create"),
            OsString::from("--draft"),
        ];
        assert_eq!(
            child(
                &binary,
                &args,
                "scoped-test-token",
                &dir.path().join("config")
            )
            .unwrap(),
            0
        );
        assert_eq!(std::fs::read_to_string(proof).unwrap(), "pr create --draft");
        assert!(!format!("{args:?}").contains("scoped-test-token"));
        assert!(
            std::fs::read_dir(dir.path().join("config"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    #[test]
    fn gh_cli_exec_fixture_child() {
        let Some(binary) = std::env::var_os("ZUSE_TEST_GH_BINARY") else {
            return;
        };
        let config = std::env::var_os("ZUSE_TEST_GH_CONFIG").unwrap();
        exec_child(
            Path::new(&binary),
            &[OsString::from("pr"), OsString::from("create")],
            "scoped-test-token",
            Path::new(&config),
        )
        .unwrap();
    }
    #[test]
    fn gh_launcher_exec_keeps_the_owned_pid_and_does_not_leave_a_cli_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("gh");
        let proof = dir.path().join("pid");
        std::fs::write(&binary,format!("#!/bin/sh\n[ \"$GH_TOKEN\" = scoped-test-token ] || exit 41\n[ -z \"${{GITHUB_TOKEN+x}}\" ] || exit 42\nprintf '%s' \"$$\" > '{}'\n",proof.display())).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "github::cli::tests::gh_cli_exec_fixture_child",
                "--exact",
                "--nocapture",
            ])
            .env("ZUSE_TEST_GH_BINARY", &binary)
            .env("ZUSE_TEST_GH_CONFIG", dir.path().join("config"))
            .env("GITHUB_TOKEN", "foreign-parent-token")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(child.wait().unwrap().success());
        assert_eq!(std::fs::read_to_string(proof).unwrap(), pid.to_string());
    }
    #[test]
    fn profile_pointer_rejects_symlinks_and_public_files() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::Builder::new()
            .prefix("zuse-git-")
            .tempdir()
            .unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let launcher = bin.join("gh");
        std::fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let marker = dir.path().join("profile");
        std::fs::write(
            &marker,
            serde_json::to_vec(&json!({"version":1,"launcher":launcher})).unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600)).unwrap();
        let uid = std::fs::metadata(&marker).unwrap().uid();
        assert_eq!(profile_launcher_at(&marker, uid).unwrap(), launcher);
        assert!(profile_launcher_at(&marker, uid + 1).is_err());
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(profile_launcher_at(&marker, uid).is_err());
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600)).unwrap();
        let target = bin.join("real");
        std::fs::rename(&launcher, &target).unwrap();
        std::os::unix::fs::symlink(&target, &launcher).unwrap();
        assert!(profile_launcher_at(&marker, uid).is_err());
    }
}

const PROFILE: &str = "/home/zuse/.zuse-github-cli";
fn profile_launcher_at(marker: &Path, uid: u32) -> Result<PathBuf> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(marker)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == uid
            && metadata.nlink() == 1
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.len() <= 4096,
        "github_cli_profile_invalid"
    );
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    let value: Value = serde_json::from_slice(&bytes)?;
    ensure!(value["version"] == 1, "github_cli_profile_invalid");
    let path = PathBuf::from(
        value["launcher"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("github_cli_profile_invalid"))?,
    );
    ensure!(
        path.is_absolute() && path.file_name() == Some(std::ffi::OsStr::new("gh")),
        "github_cli_profile_invalid"
    );
    let bin = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("github_cli_profile_invalid"))?;
    let root = bin
        .parent()
        .ok_or_else(|| anyhow::anyhow!("github_cli_profile_invalid"))?;
    ensure!(
        bin.file_name() == Some(std::ffi::OsStr::new("bin"))
            && root
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("zuse-git-")),
        "github_cli_profile_invalid"
    );
    for (entry, is_directory) in [(path.as_path(), false), (bin, true), (root, true)] {
        let stat = std::fs::symlink_metadata(entry)?;
        ensure!(
            !stat.file_type().is_symlink()
                && stat.uid() == uid
                && stat.permissions().mode() & 0o077 == 0
                && (if is_directory {
                    stat.is_dir()
                } else {
                    stat.is_file()
                }),
            "github_cli_profile_invalid"
        );
    }
    Ok(path)
}
pub fn run_profile(args: &[OsString]) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    if args.len() == 1 && matches!(args[0].to_str(), Some("--version" | "-v")) {
        managed_cli_version()?;
        return Err(std::process::Command::new(MANAGED_GH)
            .arg("--version")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .exec()
            .into());
    }
    let path = profile_launcher_at(Path::new(PROFILE), 10001)?;
    Err(std::process::Command::new(path).args(args).exec().into())
}
pub(super) fn clear_profile(directory: &Path) {
    if let Ok(value) = std::fs::read(PROFILE)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(std::io::Error::other))
    {
        if value["launcher"]
            .as_str()
            .is_some_and(|p| Path::new(p).starts_with(directory))
        {
            let _ = std::fs::remove_file(PROFILE);
        }
    }
}
