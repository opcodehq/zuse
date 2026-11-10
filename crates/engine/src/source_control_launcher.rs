//! A managed engine can require its own credentialed GitHub launcher. Only a
//! private executable path crosses this boundary; credentials stay in the child.
use super::*;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
#[derive(Default)]
struct State {
    required: AtomicBool,
    executable: OnceLock<PathBuf>,
}
#[derive(Clone, Default)]
pub struct GitHubCommandLauncher(Arc<State>);
impl GitHubCommandLauncher {
    pub(super) fn is_required(&self) -> bool {
        self.0.required.load(Ordering::Acquire)
    }
    pub fn require(&self) {
        self.0.required.store(true, Ordering::Release);
    }
    pub fn install(&self, path: PathBuf) -> Result<(), ChangeRequestError> {
        self.require();
        if !path.is_absolute()
            || !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file())
        {
            return Err(ChangeRequestError::Authentication);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&path)
                .map_err(|_| ChangeRequestError::Authentication)?
                .permissions()
                .mode()
                & 0o100
                == 0
            {
                return Err(ChangeRequestError::Authentication);
            }
            if std::fs::metadata(&path)
                .map_err(|_| ChangeRequestError::Authentication)?
                .permissions()
                .mode()
                & 0o077
                != 0
            {
                return Err(ChangeRequestError::Authentication);
            }
        }
        if self.0.executable.get().is_some_and(|old| old == &path) {
            return Ok(());
        }
        self.0
            .executable
            .set(path)
            .map_err(|_| ChangeRequestError::Authentication)
    }
    fn executable(&self) -> Result<Option<&Path>, ProcessRunError> {
        match self.0.executable.get() {
            Some(path) => Ok(Some(path)),
            None if self.0.required.load(Ordering::Acquire) => Err(ProcessRunError::Authentication),
            None => Ok(None),
        }
    }
}
struct ManagedGithubRunner {
    launcher: GitHubCommandLauncher,
}
#[async_trait]
impl ProcessRunner for ManagedGithubRunner {
    async fn run(&self, mut request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
        if request.program == "gh" {
            if let Some(path) = self.launcher.executable()? {
                request.program = path
                    .to_str()
                    .ok_or(ProcessRunError::Authentication)?
                    .to_owned();
            }
        }
        SystemProcessRunner.run(request).await
    }
}
impl GitHubCli {
    pub(crate) fn with_launcher(mut self, launcher: GitHubCommandLauncher) -> Self {
        self.managed_launcher = Some(launcher.clone());
        self.runner = Arc::new(ManagedGithubRunner { launcher });
        self
    }
}
impl ChangeRequestResolver {
    pub(crate) fn with_github(mut self, github: GitHubCli) -> Self {
        self.github = github;
        self
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_launcher_is_shared_required_and_immutable_without_account_fallback() {
        let slot = GitHubCommandLauncher::default();
        let retained = slot.clone();
        assert!(slot.executable().unwrap().is_none());
        slot.require();
        assert_eq!(
            retained.executable().unwrap_err(),
            ProcessRunError::Authentication
        );
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gh");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        slot.install(path.clone()).unwrap();
        assert_eq!(retained.executable().unwrap(), Some(path.as_path()));
        slot.install(path.clone()).unwrap();
        let other = tmp.path().join("other");
        std::fs::copy(&path, &other).unwrap();
        assert!(slot.install(other).is_err());
        assert!(
            !format!(
                "{:?}",
                ProcessRequest {
                    program: path.to_string_lossy().into_owned(),
                    args: vec!["api".into()],
                    stdin: None,
                    cwd: None,
                    env: vec![],
                    timeout: Duration::from_secs(1),
                    output_limit: 10
                }
            )
            .contains("GH_TOKEN")
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn managed_runner_uses_only_the_installed_launcher() {
        let slot = GitHubCommandLauncher::default();
        slot.require();
        let runner = ManagedGithubRunner {
            launcher: slot.clone(),
        };
        let request = || ProcessRequest {
            program: "gh".into(),
            args: vec!["--version".into()],
            stdin: None,
            cwd: None,
            env: vec![],
            timeout: Duration::from_secs(2),
            output_limit: 1024,
        };
        assert_eq!(
            runner.run(request()).await.unwrap_err(),
            ProcessRunError::Authentication
        );
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("gh");
        std::fs::write(&executable, "#!/bin/sh\nprintf managed-launcher\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        slot.install(executable).unwrap();
        let output = runner.run(request()).await.unwrap();
        assert!(output.success);
        assert_eq!(output.stdout, b"managed-launcher");
    }
}
