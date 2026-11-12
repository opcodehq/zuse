//! Background poller: fetch the directive, verify, download at low priority,
//! stage, and attest outcomes once per update id.
use super::{
    ARTIFACT_INVALID, Config, DOWNLOAD_FAILED, Layout, State, UNAVAILABLE, stage, swap, verify,
};
use crate::Authority;
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, sync::watch};
use tokio_util::sync::CancellationToken;
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap,
    cloud_runtime_updates::{Outcome, RuntimeAttestation, UpdateDirective, Urgency},
};

pub const POLL_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Without a completed turn, a Background download waits this long after wake.
pub const FIRST_TURN_GRACE: Duration = Duration::from_secs(120);
pub const MAX_COMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
/// How long a `now` or sleep swap waits for the root helper's result.
pub const ROOT_WAIT: Duration = Duration::from_secs(120);

/// What the swap boundaries need to know about the engine.
pub trait Activity: Send + Sync {
    /// A turn is working or waiting on the user.
    fn turn_active(&self) -> bool;
    /// No leased mailbox command lacks a delivered terminal ACK.
    fn mailbox_quiescent(&self) -> bool;
    fn quiescent(&self) -> bool {
        !self.turn_active() && self.mailbox_quiescent()
    }
}

/// Leased commands without an acknowledgment, or acknowledgments the server
/// has not received, block a swap. Unreadable means busy.
pub fn mailbox_quiescent(database: &Path) -> bool {
    let Ok(db) =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return false;
    };
    let _ = db.busy_timeout(Duration::from_secs(2));
    let table: rusqlite::Result<i64> = db.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='zuse_cloud_command_receipts'",
        [],
        |row| row.get(0),
    );
    match table {
        Ok(0) => true,
        Ok(_) => db
            .query_row(
                "SELECT COUNT(*) FROM zuse_cloud_command_receipts WHERE acknowledgment IS NULL OR delivered=0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .is_ok_and(|open| open == 0),
        Err(_) => false,
    }
}

/// A verified release staged and waiting for its boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ready {
    pub update_id: String,
    pub sha256: String,
    pub urgency: Urgency,
}

pub struct Updater {
    config: Config,
    /// SHA-256 of the image this process runs.
    running: String,
    capabilities: Vec<String>,
    ready: watch::Sender<Option<Ready>>,
    poll: tokio::sync::Notify,
    woke: Mutex<Instant>,
    download: reqwest::Client,
}
impl Updater {
    pub fn new(config: Config, running: String, capabilities: Vec<String>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            config,
            running,
            capabilities,
            ready: watch::channel(None).0,
            poll: tokio::sync::Notify::new(),
            woke: Mutex::new(Instant::now()),
            // Presigned object URL: never send the runtime credential, never follow.
            download: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(10 * 60))
                .build()?,
        }))
    }
    /// Production configuration for the serving process.
    pub fn detect(data: &Path) -> Result<Arc<Self>> {
        Self::new(
            Config::detect(data)?,
            swap::running_sha()?,
            crate::identity::bootstrap_capabilities(),
        )
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn poll_now(&self) {
        self.poll.notify_one();
    }
    /// After a memory resume: restart the first-turn clock and poll.
    pub fn woke(&self) {
        *self
            .woke
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        self.poll_now();
    }
    pub fn ready(&self) -> Option<Ready> {
        self.ready.borrow().clone()
    }
    /// A verified release is staged and still on disk. At a memory pause the
    /// runtime then skips memory sleep so the next start swaps at cold boot.
    pub fn staged_for_next_start(&self) -> bool {
        self.ready()
            .is_some_and(|ready| stage::load(&self.config, &ready.sha256).is_some())
    }

    pub async fn run(
        self: Arc<Self>,
        authority: Authority,
        boot: watch::Receiver<CloudRuntimeBootstrap>,
        activity: Arc<dyn Activity>,
        cancel: CancellationToken,
    ) {
        let nudges = zuse_rpc::cloud::runtime_update_poll_requests();
        loop {
            if cancel.is_cancelled() {
                return;
            }
            self.attest_pending(&authority, &boot).await;
            let current = boot.borrow().clone();
            match authority.runtime_update(&current).await {
                Ok(offer) => match offer.directive {
                    Some(directive) => {
                        self.handle(&authority, &boot, activity.as_ref(), &cancel, directive)
                            .await
                    }
                    None => {
                        self.ready.send_replace(None);
                    }
                },
                // Older servers have no route; keep polling quietly.
                Err(error) => tracing::debug!(%error, "runtime_update_poll_failed"),
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = self.poll.notified() => {}
                _ = nudges.notified() => {}
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    async fn handle(
        &self,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
        activity: &dyn Activity,
        cancel: &CancellationToken,
        directive: UpdateDirective,
    ) {
        let id = directive.update_id.clone();
        let release = directive.artifact.release.clone();
        let state = State::load(&self.config);
        if state.attested.contains(&id)
            || state.rejected.contains(&id)
            || state.rejected_urls.contains(&directive.artifact.url)
        {
            return;
        }
        if release.sha256 == self.running {
            // Already serving it (for example after a lost pending record).
            self.attest(
                authority,
                boot,
                &id,
                Outcome::Committed,
                None,
                None,
                &release.version,
            )
            .await;
            return;
        }
        if stage::read_pending(&self.config).is_some_and(|p| p.update_id == id) {
            return;
        }
        if self.config.layout == Layout::Unsupported {
            self.refuse(authority, boot, &directive, UNAVAILABLE).await;
            return;
        }
        if let Err(error) = verify::release(
            &release,
            &directive.artifact.signature,
            &self.config.verifying_key,
            &self.config.architecture,
        ) {
            let code = known_code(&error.to_string());
            self.refuse(authority, boot, &directive, code).await;
            return;
        }
        let ready = Ready {
            update_id: id.clone(),
            sha256: release.sha256.clone(),
            urgency: directive.urgency,
        };
        if stage::load(&self.config, &release.sha256).is_some_and(|s| s.file.update_id == id) {
            self.ready.send_replace(Some(ready));
            return;
        }
        if directive.urgency == Urgency::Background {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = self.background_gate(activity) => {}
            }
        }
        if directive.artifact.url_expires_at <= crate::now_ms() + 5_000 {
            // Poll again for a fresh URL.
            self.poll_now();
            return;
        }
        let downloaded = tokio::select! {
            _ = cancel.cancelled() => return,
            result = self.download(&directive) => result,
        };
        match downloaded {
            Ok(_) => {
                tracing::info!(update = %id, "Runtime update staged");
                self.ready.send_replace(Some(ready));
            }
            Err(code) => self.refuse(authority, boot, &directive, code).await,
        }
    }

    /// Never during the first turn after wake: wait until a turn finished, or
    /// the computer stayed idle for [`FIRST_TURN_GRACE`].
    async fn background_gate(&self, activity: &dyn Activity) {
        let mut saw_turn = false;
        loop {
            let since_wake = self
                .woke
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .elapsed();
            if activity.turn_active() {
                saw_turn = true;
            } else if saw_turn || since_wake >= FIRST_TURN_GRACE {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Download, decompress and verify into `staged/<sha256>`. Errors are codes.
    pub async fn download(
        &self,
        directive: &UpdateDirective,
    ) -> std::result::Result<stage::Staged, &'static str> {
        let artifact = &directive.artifact;
        let url = reqwest::Url::parse(&artifact.url).map_err(|_| ARTIFACT_INVALID)?;
        let loopback = url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
            || artifact.compressed_size_bytes == 0
            || artifact.compressed_size_bytes > MAX_COMPRESSED_BYTES
        {
            return Err(ARTIFACT_INVALID);
        }
        let dir = stage::incoming(&self.config).map_err(|_| DOWNLOAD_FAILED)?;
        let result = async {
            let compressed = dir.join("artifact.zst");
            let mut response = self
                .download
                .get(url)
                .send()
                .await
                .map_err(|_| DOWNLOAD_FAILED)?;
            if !response.status().is_success() {
                return Err(DOWNLOAD_FAILED);
            }
            let mut file = tokio::fs::File::create(&compressed)
                .await
                .map_err(|_| DOWNLOAD_FAILED)?;
            let mut total = 0u64;
            while let Some(chunk) = response.chunk().await.map_err(|_| DOWNLOAD_FAILED)? {
                total += chunk.len() as u64;
                if total > artifact.compressed_size_bytes {
                    return Err(ARTIFACT_INVALID);
                }
                file.write_all(&chunk).await.map_err(|_| DOWNLOAD_FAILED)?;
            }
            file.sync_all().await.map_err(|_| DOWNLOAD_FAILED)?;
            drop(file);
            if total != artifact.compressed_size_bytes {
                return Err(DOWNLOAD_FAILED);
            }
            // A dedicated low-priority thread: a reused blocking-pool thread
            // would keep the lowered priority for unrelated work.
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let (config, release, dir2) =
                (self.config.clone(), artifact.release.clone(), dir.clone());
            let file = stage::ReleaseFile {
                release: artifact.release.clone(),
                signature: artifact.signature.clone(),
                update_id: directive.update_id.clone(),
            };
            std::thread::Builder::new()
                .name("runtime-update".into())
                .spawn(move || {
                    lower_priority();
                    let staged = stage::decompress(&compressed, &dir2, &release).and_then(|b| {
                        let _ = std::fs::remove_file(&compressed);
                        stage::stage(&config, file, &b)
                    });
                    let _ = sender.send(staged);
                })
                .map_err(|_| DOWNLOAD_FAILED)?;
            match receiver.await.map_err(|_| DOWNLOAD_FAILED)? {
                Ok(staged) => Ok(staged),
                Err(error) if error.to_string() == ARTIFACT_INVALID => Err(ARTIFACT_INVALID),
                Err(_) => Err(DOWNLOAD_FAILED),
            }
        }
        .await;
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// Resolves once a `now` release is staged and the engine is quiescent.
    pub async fn now_ready(&self, activity: &dyn Activity) -> Ready {
        let mut changes = self.ready.subscribe();
        loop {
            let ready = changes.borrow_and_update().clone();
            if let Some(ready) = ready {
                if ready.urgency == Urgency::Now && activity.quiescent() {
                    return ready;
                }
            }
            tokio::select! {
                _ = changes.changed() => {}
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    /// Install the ready release. The caller proved quiescence and closed
    /// execution admission. Returns the binary to exec.
    pub async fn install(&self, cancel: &CancellationToken) -> Option<PathBuf> {
        let ready = self.ready()?;
        let staged = stage::load(&self.config, &ready.sha256)?;
        let (config, running, token) = (self.config.clone(), self.running.clone(), cancel.clone());
        let installed = tokio::task::spawn_blocking(move || {
            swap::install(&config, &staged, &running, ROOT_WAIT, &|| {
                token.is_cancelled()
            })
        })
        .await;
        self.ready.send_replace(None);
        match installed {
            Ok(Ok(Some(binary))) => Some(binary),
            Ok(Ok(None)) => {
                // Attest whatever the helper or journal decided.
                self.poll_now();
                None
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "runtime_update_install_failed");
                self.poll_now();
                None
            }
            Err(_) => None,
        }
    }

    /// Attest the outcome of a swap this runtime (or its predecessor) requested.
    async fn attest_pending(
        &self,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
    ) {
        let Some(pending) = stage::read_pending(&self.config) else {
            return;
        };
        let Some((outcome, code)) =
            swap::attestation_outcome(&self.config, &pending, &self.running)
        else {
            return;
        };
        let (previous, version) = if outcome == Outcome::Committed {
            (
                Some(pending.previous_sha256.clone()),
                pending.version.clone(),
            )
        } else {
            (None, env!("CARGO_PKG_VERSION").to_owned())
        };
        if self
            .attest(
                authority,
                boot,
                &pending.update_id,
                outcome,
                code,
                previous,
                &version,
            )
            .await
        {
            stage::clear_pending(&self.config);
            if outcome == Outcome::Committed {
                stage::discard_others(&self.config, None);
            } else {
                let mut state = State::load(&self.config);
                State::push(&mut state.rejected, &pending.update_id);
                let _ = state.save(&self.config);
                stage::discard(&self.config, &pending.sha256);
            }
        }
    }

    async fn refuse(
        &self,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
        directive: &UpdateDirective,
        code: &'static str,
    ) {
        tracing::warn!(update = %directive.update_id, code, "runtime_update_refused");
        let mut state = State::load(&self.config);
        State::push(&mut state.rejected, &directive.update_id);
        if code != DOWNLOAD_FAILED {
            // Never retry the same URL after a digest or signature failure.
            State::push(&mut state.rejected_urls, &directive.artifact.url);
        }
        let _ = state.save(&self.config);
        self.attest(
            authority,
            boot,
            &directive.update_id,
            Outcome::Failed,
            Some(code.into()),
            None,
            env!("CARGO_PKG_VERSION"),
        )
        .await;
    }

    /// POST once per update id; `true` once delivered (or already delivered).
    #[allow(clippy::too_many_arguments)]
    async fn attest(
        &self,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
        update_id: &str,
        outcome: Outcome,
        error_code: Option<String>,
        previous_sha256: Option<String>,
        version: &str,
    ) -> bool {
        if State::load(&self.config)
            .attested
            .iter()
            .any(|id| id == update_id)
        {
            return true;
        }
        let attestation = RuntimeAttestation {
            update_id: Some(update_id.into()),
            outcome,
            sha256: self.running.clone(),
            version: version.into(),
            capabilities: self.capabilities.clone(),
            previous_sha256,
            error_code,
        };
        for attempt in 0..3 {
            let current = boot.borrow().clone();
            match authority
                .runtime_update_attestation(&current, &attestation)
                .await
            {
                Ok(()) => {
                    let mut state = State::load(&self.config);
                    State::push(&mut state.attested, update_id);
                    let _ = state.save(&self.config);
                    return true;
                }
                Err(error) if crate::supervisor::retryable(&error) && attempt < 2 => {
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                }
                Err(error) => {
                    tracing::warn!(%error, "runtime_update_attestation_failed");
                    return false;
                }
            }
        }
        false
    }
}

fn known_code(message: &str) -> &'static str {
    match message {
        super::STORAGE_INCOMPATIBLE => super::STORAGE_INCOMPATIBLE,
        _ => ARTIFACT_INVALID,
    }
}

fn lower_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        // Raising one's own nice value is always permitted; this thread only.
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}
