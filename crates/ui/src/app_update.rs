//! Desktop self-update: the app's own release checker plus the download /
//! install lifecycle behind the sidebar update strip, "Check for Updates…"
//! (macOS app menu; the account menu elsewhere) and install-on-quit.
//!
//! The checker runs in this process against this binary's version. The
//! engine keeps its own checker for daemons and remote reporting, but the UI
//! may be attached to a daemon of another version, whose status describes
//! that binary rather than the app the user is looking at.
//!
//! Lifecycle, idiomatic to desktop updaters (Sparkle, VS Code, Zed): a found
//! release downloads in the background and is verified, the strip offers
//! "restart to apply", and a staged update the user never restarts for is
//! installed when the app quits — so nobody stays on a stale version just
//! because they never clicked. `ZUSE_AUTO_UPDATE=0` keeps it report-only.

use std::path::{Path, PathBuf};

use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use gpui_tokio::Tokio;
use zuse_update::{InstallKind, UpdateBlocker, UpdateStatus, Updater};

/// Download/install lifecycle of the newest release in this process.
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    Idle,
    Downloading {
        version: String,
    },
    /// Staged and verified: "restart to apply", or installed on quit.
    Ready {
        version: String,
        staged: PathBuf,
    },
    Failed {
        version: String,
        message: SharedString,
    },
    /// Swapped in; this process is on its way out.
    Installed,
}

/// How the install attempted before this launch turned out.
#[derive(Debug, Clone, PartialEq, Eq)]
enum InstallOutcome {
    Applied(String),
    NotApplied(String),
}

fn install_outcome(attempted: &str, current: &str) -> Option<InstallOutcome> {
    if attempted == current {
        Some(InstallOutcome::Applied(attempted.to_owned()))
    } else if zuse_update::version_newer(attempted, current) {
        Some(InstallOutcome::NotApplied(attempted.to_owned()))
    } else {
        // Already past it, by some other route: nothing to report.
        None
    }
}

/// The flow after a background stage finishes. A failed download of something
/// newer leaves the update already ready in place: it can still be installed.
fn flow_after_stage(
    previous: Flow,
    requested: String,
    outcome: Result<(String, PathBuf), String>,
) -> Flow {
    match (previous, outcome) {
        // Already restarting into another version.
        (Flow::Installed, _) => Flow::Installed,
        (_, Ok((version, staged))) => Flow::Ready { version, staged },
        (ready @ Flow::Ready { .. }, Err(message)) => {
            tracing::warn!(%message, "downloading a newer update failed; keeping the ready one");
            ready
        }
        (_, Err(message)) => {
            tracing::warn!(%message, "update download failed");
            Flow::Failed {
                version: requested,
                message: message.into(),
            }
        }
    }
}

fn install_attempt_path(data_dir: &Path) -> PathBuf {
    data_dir.join("update-install-attempt")
}

/// Noted before the swap: part of an install happens after this process has
/// exited, where only the next launch can see whether it worked.
fn record_install_attempt(data_dir: &Path, version: &str) {
    if let Err(error) = std::fs::write(install_attempt_path(data_dir), version) {
        tracing::warn!(%error, "could not record the update attempt");
    }
}

fn clear_install_attempt(data_dir: &Path) {
    let _ = std::fs::remove_file(install_attempt_path(data_dir));
}

/// The version the previous run tried to install. Reported once.
fn take_install_attempt(data_dir: &Path) -> Option<String> {
    let attempted = std::fs::read_to_string(install_attempt_path(data_dir)).ok()?;
    clear_install_attempt(data_dir);
    Some(attempted.trim().to_owned()).filter(|version| !version.is_empty())
}

/// The user-initiated check's dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum Prompt {
    Checking,
    /// The check finished; the dialog renders from the live status + flow.
    Result,
    CheckFailed(SharedString),
}

/// What clicking the sidebar strip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripAction {
    /// Nothing — progress only.
    None,
    Download,
    Restart,
    /// Explain why this install can't update itself (dialog).
    Explain,
    /// Advisory installs: open the releases page (unmanaged) and dismiss.
    Advise {
        open_releases: bool,
    },
}

pub struct AppUpdate {
    install: InstallKind,
    blocker: Option<UpdateBlocker>,
    /// Background download + install on quit (`ZUSE_AUTO_UPDATE` unset or on).
    automatic: bool,
    edge_url: String,
    data_dir: PathBuf,
    checker: Updater,
    status: Option<UpdateStatus>,
    flow: Flow,
    prompt: Option<Prompt>,
    /// Version whose advisory strip the user dismissed (a newer release shows
    /// it again).
    dismissed: Option<String>,
    _status_watch: Task<()>,
    download: Option<Task<()>>,
    user_check: Option<Task<()>>,
    /// The offered release's changelog, keyed by its version.
    release_notes: Option<(String, Vec<zuse_update::ReleaseNotesSection>)>,
    notes_fetch: Option<(String, Task<()>)>,
    /// A version that failed to install last launch: it waits for Retry
    /// rather than downloading and failing again on its own.
    not_applied: Option<String>,
    /// A newer release is downloading behind a flow that stays `Ready`.
    staging_newer: bool,
}

struct GlobalAppUpdate(Entity<AppUpdate>);

impl Global for GlobalAppUpdate {}

impl AppUpdate {
    /// Start the desktop checker. Call once at boot, after `gpui_tokio` is
    /// initialized.
    pub fn init(edge_url: String, data_dir: PathBuf, cx: &mut App) {
        let entity = cx.new(|cx| Self::new(edge_url, data_dir, cx));
        cx.set_global(GlobalAppUpdate(entity));
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAppUpdate>()
            .map(|global| global.0.clone())
    }

    fn new(edge_url: String, data_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let install = zuse_update::detect_install();
        let blocker = install
            .supports_desktop_update()
            .then(|| install.desktop_update_blocker())
            .flatten();
        let checker = {
            let _runtime = Tokio::handle(cx).enter();
            Updater::spawn_desktop(edge_url.clone())
        };
        let mut statuses = checker.watch();
        let status_watch = cx.spawn(async move |this, cx| {
            while statuses.changed().await.is_ok() {
                let status = statuses.borrow_and_update().clone();
                if this
                    .update(cx, |this, cx| this.apply_status(status, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        tracing::info!(?install, ?blocker, "desktop update checker started");
        let outcome = take_install_attempt(&data_dir)
            .and_then(|attempted| install_outcome(&attempted, zuse_update::current_version()));
        let mut flow = Flow::Idle;
        let mut not_applied = None;
        match outcome {
            Some(InstallOutcome::Applied(version)) => {
                crate::toast::success(cx, format!("Updated to v{version}"));
            }
            Some(InstallOutcome::NotApplied(version)) => {
                tracing::warn!(%version, "the previous update did not install");
                crate::toast::error(
                    cx,
                    format!("v{version} didn't install. Use Retry in the sidebar to try again."),
                );
                flow = Flow::Failed {
                    message: format!("v{version} didn't install. Click to retry.").into(),
                    version: version.clone(),
                };
                not_applied = Some(version);
            }
            None => {}
        }
        Self {
            install,
            blocker,
            automatic: zuse_update::desktop_auto_update_enabled(),
            edge_url,
            data_dir,
            checker,
            status: None,
            flow,
            prompt: None,
            dismissed: None,
            _status_watch: status_watch,
            download: None,
            user_check: None,
            release_notes: None,
            notes_fetch: None,
            not_applied,
            staging_newer: false,
        }
    }

    pub fn install(&self) -> &InstallKind {
        &self.install
    }

    pub fn blocker(&self) -> Option<&UpdateBlocker> {
        self.blocker.as_ref()
    }

    pub fn flow(&self) -> &Flow {
        &self.flow
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// The newer release the last check found, if any.
    pub fn available(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|status| status.update_available)
            .and_then(|status| status.latest_version.as_deref())
    }

    /// The offered version with its changelog, once that has loaded.
    pub fn release_notes(&self) -> Option<(&str, &[zuse_update::ReleaseNotesSection])> {
        let offered = match &self.flow {
            Flow::Ready { version, .. } => version.as_str(),
            _ => self.available()?,
        };
        let (version, sections) = self.release_notes.as_ref()?;
        (version == offered).then_some((offered, sections.as_slice()))
    }

    /// Fetch the offered release's changelog for the update pill. A failure
    /// only costs the hover card; the next status change tries again.
    fn load_release_notes(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.available().map(str::to_owned) else {
            return;
        };
        if self
            .release_notes
            .as_ref()
            .is_some_and(|(v, _)| *v == version)
            || self
                .notes_fetch
                .as_ref()
                .is_some_and(|(v, _)| *v == version)
        {
            return;
        }
        let requested = version.clone();
        let fetch = Tokio::spawn(cx, async move {
            zuse_update::fetch_release_notes(&requested).await
        });
        let loaded = version.clone();
        let task = cx.spawn(async move |this, cx| {
            let sections = match fetch.await {
                Ok(Ok(sections)) => sections,
                Ok(Err(err)) => {
                    tracing::debug!(error = %format!("{err:#}"), "release notes unavailable");
                    Vec::new()
                }
                Err(join) => {
                    tracing::debug!(error = %join, "release notes unavailable");
                    Vec::new()
                }
            };
            this.update(cx, |this, cx| {
                this.notes_fetch = None;
                if !sections.is_empty() {
                    this.release_notes = Some((loaded, sections));
                    cx.notify();
                }
            })
            .ok();
        });
        self.notes_fetch = Some((version, task));
    }

    /// Whether this app downloads and installs updates itself right now.
    pub fn self_updating(&self) -> bool {
        self.install.supports_desktop_update() && self.blocker.is_none()
    }

    /// Window activation / wake: check if one is due by the wall clock.
    pub fn poke(&self) {
        self.checker.poke();
    }

    fn apply_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        let succeeded = status.error.is_none();
        self.status = Some(status);
        self.load_release_notes(cx);
        if succeeded && self.automatic {
            self.download_if_needed(cx);
        }
        cx.notify();
    }

    /// Start (or restart) the background download unless the newest release
    /// is already downloading or staged. A failed download retries at the
    /// next successful check.
    fn download_if_needed(&mut self, cx: &mut Context<Self>) {
        if !self.self_updating() {
            return;
        }
        let Some(latest) = self.available() else {
            return;
        };
        if self.not_applied.as_deref() == Some(latest) {
            return;
        }
        let current = match &self.flow {
            Flow::Downloading { .. } | Flow::Installed => return,
            Flow::Ready { version, .. } => Some(version),
            Flow::Idle | Flow::Failed { .. } => None,
        };
        if current.is_some_and(|version| !zuse_update::version_newer(latest, version)) {
            return;
        }
        self.start_download(cx);
    }

    pub fn start_download(&mut self, cx: &mut Context<Self>) {
        if !self.self_updating()
            || self.staging_newer
            || matches!(self.flow, Flow::Downloading { .. } | Flow::Installed)
        {
            return;
        }
        let Some(version) = self.available().map(str::to_owned) else {
            return;
        };
        self.not_applied = None;
        let edge_url = self.edge_url.clone();
        let data_dir = self.data_dir.clone();
        let install = self.install.clone();
        // Restart stays available while a newer release downloads behind it.
        self.staging_newer = matches!(self.flow, Flow::Ready { .. });
        if !self.staging_newer {
            self.flow = Flow::Downloading {
                version: version.clone(),
            };
        }
        let stage = Tokio::spawn(cx, async move {
            // Re-read the manifest so a long-lived status still downloads the
            // newest release, with that release's checksums.
            let manifest = zuse_update::fetch_latest(&edge_url).await?;
            anyhow::ensure!(
                zuse_update::version_newer(&manifest.version, zuse_update::current_version()),
                "the release feed no longer offers a newer version"
            );
            let staged = install
                .stage_desktop(&edge_url, &manifest, &data_dir)
                .await?;
            anyhow::Ok((manifest.version, staged))
        });
        self.download = Some(cx.spawn(async move |this, cx| {
            let outcome = match stage.await {
                Ok(Ok(staged)) => Ok(staged),
                Ok(Err(err)) => Err(format!("{err:#}")),
                Err(join) => Err(join.to_string()),
            };
            this.update(cx, |this, cx| {
                this.staging_newer = false;
                let previous = std::mem::replace(&mut this.flow, Flow::Idle);
                this.flow = flow_after_stage(previous, version, outcome);
                if let Flow::Ready { version, staged } = &this.flow {
                    tracing::info!(%version, staged = %staged.display(), "update staged");
                    zuse_update::prune_stale_stages(&this.data_dir, version);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// "Check for Updates…": check now and report the outcome in a dialog. A
    /// found release starts downloading at once (it would anyway on the
    /// hourly check), and a failed download gets a fresh attempt.
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.prompt = Some(Prompt::Checking);
        let checker = self.checker.clone();
        let check = Tokio::spawn(cx, async move { checker.check().await });
        self.user_check = Some(cx.spawn(async move |this, cx| {
            let result = match check.await {
                Ok(Ok(status)) => Ok(status),
                Ok(Err(err)) => {
                    tracing::warn!(error = %format!("{err:#}"), "update check failed");
                    // The dialog shows the cause; the log keeps the chain.
                    Err(err.root_cause().to_string())
                }
                Err(join) => Err(join.to_string()),
            };
            this.update(cx, |this, cx| {
                let outcome = match result {
                    Ok(status) => {
                        this.status = Some(status);
                        this.load_release_notes(cx);
                        if this.automatic {
                            this.download_if_needed(cx);
                        }
                        Prompt::Result
                    }
                    Err(message) => Prompt::CheckFailed(message.into()),
                };
                // The user may have closed the dialog while it was checking.
                if this.prompt.is_some() {
                    this.prompt = Some(outcome);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn dismiss_prompt(&mut self, cx: &mut Context<Self>) {
        self.prompt = None;
        self.user_check = None;
        cx.notify();
    }

    /// Open the dialog on its current result (the strip's "explain" action).
    pub fn show_result(&mut self, cx: &mut Context<Self>) {
        self.prompt = Some(Prompt::Result);
        cx.notify();
    }

    /// The staged update "Restart to update" would install.
    pub fn staged(&self) -> Option<PathBuf> {
        match &self.flow {
            Flow::Ready { staged, .. } => Some(staged.clone()),
            _ => None,
        }
    }

    /// Install `staged` and arrange the relaunch. The caller quits on success.
    pub fn install_for_restart(
        &mut self,
        staged: &Path,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if let Flow::Ready { version, .. } = &self.flow {
            record_install_attempt(&self.data_dir, version);
        }
        let result = self.install.apply_desktop(staged, true);
        if result.is_err() {
            clear_install_attempt(&self.data_dir);
        }
        self.flow = match &result {
            Ok(()) => Flow::Installed,
            Err(err) => {
                tracing::error!(error = %err, "update apply failed");
                Flow::Failed {
                    version: self.available().unwrap_or_default().to_owned(),
                    message: format!("{err:#}").into(),
                }
            }
        };
        cx.notify();
        result
    }

    /// Quit hook: a staged update the user never restarted for installs now,
    /// so the next launch is current. Runs synchronously inside the quit.
    fn install_on_quit(&mut self) {
        if !self.automatic {
            return;
        }
        let Flow::Ready { version, staged } = &self.flow else {
            return;
        };
        record_install_attempt(&self.data_dir, version);
        match self.install.apply_desktop(staged, false) {
            Ok(()) => {
                tracing::info!(%version, "installed staged update on quit");
                self.flow = Flow::Installed;
            }
            Err(err) => {
                clear_install_attempt(&self.data_dir);
                tracing::warn!(error = %err, "installing the staged update on quit failed")
            }
        }
    }

    pub fn dismiss_advisory(&mut self, cx: &mut Context<Self>) {
        self.dismissed = self.available().map(str::to_owned);
        cx.notify();
    }

    /// The sidebar strip for the current state: `None` while there is
    /// nothing newer (or the advisory was dismissed for this version).
    pub fn strip(&self) -> Option<(SharedString, StripAction)> {
        let latest = self.available()?;
        if self.dismissed.as_deref() == Some(latest) {
            return None;
        }
        Some(strip_for(
            &self.install,
            self.blocker.is_some(),
            &self.flow,
            latest,
        ))
    }
}

/// Label + click action of the strip. Self-updating installs drive their flow
/// from it; blocked installs explain themselves; managed installs without a
/// desktop path get the `zuse update` hint; unmanaged installs (source builds,
/// hand-copied binaries) are pointed at the GitHub releases page.
pub fn strip_for(
    install: &InstallKind,
    blocked: bool,
    flow: &Flow,
    latest: &str,
) -> (SharedString, StripAction) {
    if install.supports_desktop_update() {
        if blocked {
            return (
                format!("Update available — v{latest}").into(),
                StripAction::Explain,
            );
        }
        return match flow {
            Flow::Idle => (
                format!("Update available — v{latest}").into(),
                StripAction::Download,
            ),
            Flow::Downloading { version } => {
                (format!("Downloading v{version}…").into(), StripAction::None)
            }
            Flow::Ready { version, .. } => (
                format!("Zuse v{version} is ready").into(),
                StripAction::Restart,
            ),
            Flow::Failed { message, .. } => (
                format!("Update failed: {message}").into(),
                StripAction::Download,
            ),
            Flow::Installed => ("Restarting…".into(), StripAction::None),
        };
    }
    if matches!(install, InstallKind::Managed { .. }) {
        (
            format!("Update available — v{latest} · run `zuse update`").into(),
            StripAction::Advise {
                open_releases: false,
            },
        )
    } else {
        (
            format!("Update available — v{latest} · download from GitHub").into(),
            StripAction::Advise {
                open_releases: true,
            },
        )
    }
}

/// "Check for Updates…" from a menu: surface the main window, then check.
pub fn check_for_updates(cx: &mut App) {
    crate::activate_main_window(cx);
    if let Some(update) = AppUpdate::global(cx) {
        update.update(cx, |update, cx| update.check_for_updates(cx));
    }
}

/// See [`AppUpdate::install_on_quit`].
pub fn install_on_quit(cx: &mut App) {
    if let Some(update) = AppUpdate::global(cx) {
        update.update(cx, |update, _| update.install_on_quit());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(version: &str) -> Flow {
        Flow::Ready {
            version: version.into(),
            staged: PathBuf::from(format!("/stage/{version}")),
        }
    }

    #[test]
    fn a_failed_newer_download_keeps_the_ready_update() {
        assert_eq!(
            flow_after_stage(ready("0.24.0"), "0.25.0".into(), Err("offline".into())),
            ready("0.24.0")
        );
    }

    #[test]
    fn a_verified_newer_download_replaces_the_ready_update() {
        let staged = PathBuf::from("/stage/0.25.0");
        assert_eq!(
            flow_after_stage(
                ready("0.24.0"),
                "0.25.0".into(),
                Ok(("0.25.0".into(), staged))
            ),
            ready("0.25.0")
        );
    }

    #[test]
    fn a_first_download_failure_is_reported() {
        assert_eq!(
            flow_after_stage(
                Flow::Downloading {
                    version: "0.25.0".into()
                },
                "0.25.0".into(),
                Err("offline".into())
            ),
            Flow::Failed {
                version: "0.25.0".into(),
                message: "offline".into()
            }
        );
    }

    #[test]
    fn a_download_finishing_during_restart_changes_nothing() {
        assert_eq!(
            flow_after_stage(
                Flow::Installed,
                "0.25.0".into(),
                Ok(("0.25.0".into(), PathBuf::from("/stage/0.25.0")))
            ),
            Flow::Installed
        );
    }

    #[test]
    fn the_next_launch_reports_how_the_install_went() {
        assert_eq!(
            install_outcome("0.27.0", "0.27.0"),
            Some(InstallOutcome::Applied("0.27.0".into()))
        );
        assert_eq!(
            install_outcome("0.27.0", "0.26.0"),
            Some(InstallOutcome::NotApplied("0.27.0".into()))
        );
        assert_eq!(install_outcome("0.27.0", "0.28.0"), None);
    }

    #[test]
    fn an_install_attempt_is_reported_only_once() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(take_install_attempt(dir.path()), None);
        record_install_attempt(dir.path(), "0.27.0");
        assert_eq!(take_install_attempt(dir.path()).as_deref(), Some("0.27.0"));
        assert_eq!(take_install_attempt(dir.path()), None);
    }

    #[test]
    fn strip_follows_the_flow_on_self_updating_installs() {
        let mac_app = InstallKind::MacApp {
            bundle: PathBuf::from("/Applications/Zuse.app"),
        };
        assert_eq!(
            strip_for(&mac_app, false, &Flow::Idle, "0.2.86"),
            (
                SharedString::from("Update available — v0.2.86"),
                StripAction::Download
            )
        );
        let downloading = Flow::Downloading {
            version: "0.2.86".into(),
        };
        assert_eq!(
            strip_for(&mac_app, false, &downloading, "0.2.86"),
            (
                SharedString::from("Downloading v0.2.86…"),
                StripAction::None
            )
        );
        let ready = Flow::Ready {
            version: "0.2.86".into(),
            staged: PathBuf::from("/tmp/Zuse.app"),
        };
        assert_eq!(
            strip_for(&mac_app, false, &ready, "0.2.86").1,
            StripAction::Restart
        );
        let failed = Flow::Failed {
            version: "0.2.86".into(),
            message: "offline".into(),
        };
        assert_eq!(
            strip_for(&mac_app, false, &failed, "0.2.86"),
            (
                SharedString::from("Update failed: offline"),
                StripAction::Download
            )
        );
        // A translocated or read-only bundle explains instead of failing.
        assert_eq!(
            strip_for(&mac_app, true, &Flow::Idle, "0.2.86").1,
            StripAction::Explain
        );
    }

    #[test]
    fn strip_advises_installs_without_a_desktop_path() {
        let unmanaged = strip_for(&InstallKind::Unmanaged, false, &Flow::Idle, "0.2.86");
        assert_eq!(
            unmanaged,
            (
                SharedString::from("Update available — v0.2.86 · download from GitHub"),
                StripAction::Advise {
                    open_releases: true
                }
            )
        );
        let managed = InstallKind::Managed {
            app_root: PathBuf::from("/home/u/.zuse/app"),
        };
        let strip = strip_for(&managed, false, &Flow::Idle, "0.2.86");
        if cfg!(target_os = "linux") {
            // Linux desktop installs share the managed layout and update
            // themselves like the other desktop platforms.
            assert_eq!(strip.1, StripAction::Download);
        } else {
            assert_eq!(
                strip.0,
                SharedString::from("Update available — v0.2.86 · run `zuse update`")
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_installs_drive_the_desktop_flow() {
        let installed = InstallKind::WindowsPortable {
            directory: PathBuf::from(r"C:\Users\u\AppData\Local\Programs\Zuse"),
        };
        assert_eq!(
            strip_for(&installed, false, &Flow::Idle, "0.2.86"),
            (
                SharedString::from("Update available — v0.2.86"),
                StripAction::Download
            )
        );
    }
}
