//! Where the managed runtime lives on a computer, per provider. Most bases
//! install as root (a `zuse` task user, /usr/local, sudo). Prized has no root:
//! its base installs under /home/prized/.zuse and is driven by one entry
//! point, `zuse-launch`, which also restarts the supervisor after a cold boot.
use zuse_protocol::cloud_computers::ProviderId;

const HOME_ROOT: &str = "/home/prized/.zuse";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    Root,
    Home,
}
impl Layout {
    pub(crate) fn for_provider(provider: ProviderId) -> Self {
        match provider {
            ProviderId::Prized => Self::Home,
            _ => Self::Root,
        }
    }
    pub(crate) fn runtime_binary(self) -> &'static str {
        match self {
            Self::Root => "/usr/local/bin/zuse-cloud-runtime",
            Self::Home => "/home/prized/.zuse/bin/zuse-cloud-runtime",
        }
    }
    pub(crate) fn image_manifest(self) -> &'static str {
        match self {
            Self::Root => "/opt/zuse/preparation/status/manifest.json",
            Self::Home => "/home/prized/.zuse/preparation/status/manifest.json",
        }
    }
    /// Uploaded boot credential, before it is moved into place.
    pub(crate) fn staged_boot(self, operation: &str) -> String {
        match self {
            Self::Root => format!("/tmp/zuse-rust-boot-{operation}"),
            Self::Home => format!("{HOME_ROOT}/boot/zuse-rust-boot-{operation}"),
        }
    }
    pub(crate) fn boot_file(self, operation: &str) -> String {
        match self {
            Self::Root => format!("/var/lib/zuse/workspace/rust-boot-{operation}"),
            // The supervisor accepts boot credentials only in its status folder.
            Self::Home => format!("{HOME_ROOT}/status/rust-boot-{operation}"),
        }
    }
    /// Shell lines that move the staged credential into place, privately.
    pub(crate) fn install_boot(self, staged: &str, boot: &str) -> String {
        match self {
            Self::Root => format!(
                "sudo -n install -o zuse -g zuse -m 600 {staged} {boot}\nsudo -n rm -- {staged}\n"
            ),
            Self::Home => format!(
                "mkdir -p -m 700 {HOME_ROOT}/status\ninstall -m 600 {staged} {boot}\nrm -- {staged}\n"
            ),
        }
    }
    /// Prefix for running a command as the task user with an environment.
    pub(crate) fn run_as_task(self) -> &'static str {
        match self {
            Self::Root => "sudo -n -u zuse env",
            Self::Home => "env",
        }
    }
    pub(crate) fn launcher(self) -> &'static str {
        match self {
            Self::Root => "/usr/local/lib/zuse/launch-rust-runtime",
            Self::Home => "/home/prized/.zuse/lib/zuse-launch",
        }
    }
    /// Recovery launcher with the environment its layout needs.
    pub(crate) fn recovery_launcher(self) -> &'static str {
        match self {
            Self::Root => "/usr/local/lib/zuse/launch-rust-recovery",
            Self::Home => {
                "ZUSE_USER_DATA=/home/prized/.zuse-data ZUSE_RECOVERY_STATUS_ROOT=/home/prized/.zuse/recovery /home/prized/.zuse/lib/launch-rust-recovery"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prized_runs_from_the_home_directory_without_sudo() {
        let home = Layout::for_provider(ProviderId::Prized);
        assert_eq!(home, Layout::Home);
        assert!(home.staged_boot("op").starts_with("/home/prized/"));
        assert!(!home.install_boot("a", "b").contains("sudo"));
        assert_eq!(home.run_as_task(), "env");
        for provider in [
            ProviderId::Boxd,
            ProviderId::Boat,
            ProviderId::E2b,
            ProviderId::Daytona,
        ] {
            assert_eq!(Layout::for_provider(provider), Layout::Root);
        }
        assert_eq!(
            Layout::Root.install_boot("'s'", "'b'"),
            "sudo -n install -o zuse -g zuse -m 600 's' 'b'\nsudo -n rm -- 's'\n"
        );
    }
}
