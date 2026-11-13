//! Background poller: fetch directives, verify, download at low priority,
//! install side by side, adopt at quiescent boundaries and attest.
use super::{ARTIFACT_INVALID, BundleFile, DOWNLOAD_FAILED, Harnesses, store, verify_bundle};
use crate::Authority;
use crate::updates::Activity;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, sync::watch};
use tokio_util::sync::CancellationToken;
use zuse_protocol::{cloud::CloudRuntimeBootstrap, cloud_harness_updates::HarnessDirective};

pub const POLL_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Without a completed turn, a download waits this long after start or wake.
pub const FIRST_TURN_GRACE: Duration = Duration::from_secs(120);
pub const MAX_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
/// How often a staged bundle checks for its boundary.
pub const BOUNDARY_CHECK: Duration = Duration::from_secs(2);

pub struct Updater {
    harnesses: Arc<Harnesses>,
    started: Instant,
    download: reqwest::Client,
    first_turn_grace: Duration,
}

impl Updater {
    pub fn new(harnesses: Arc<Harnesses>) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            harnesses,
            started: Instant::now(),
            // Presigned object URL: never send the runtime credential, never follow.
            download: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(15 * 60))
                .build()?,
            first_turn_grace: FIRST_TURN_GRACE,
        }))
    }
    #[cfg(test)]
    pub(crate) fn with_grace(mut self: Arc<Self>, grace: Duration) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("unshared updater")
            .first_turn_grace = grace;
        self
    }

    pub async fn run(
        self: Arc<Self>,
        authority: Authority,
        boot: watch::Receiver<CloudRuntimeBootstrap>,
        activity: Arc<dyn Activity>,
        cancel: CancellationToken,
    ) {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            self.cycle(&authority, &boot, activity.as_ref(), &cancel)
                .await;
            // Adopt staged bundles at the first boundary before the next poll.
            let deadline = Instant::now() + POLL_INTERVAL;
            while Instant::now() < deadline {
                if self.harnesses.has_staged() && activity.quiescent() {
                    self.adopt(&authority, &boot).await;
                }
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(BOUNDARY_CHECK) => {}
                }
            }
        }
    }

    /// One poll: deliver attestations, then handle each directive.
    pub async fn cycle(
        &self,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
        activity: &dyn Activity,
        cancel: &CancellationToken,
    ) {
        self.attest(authority, boot).await;
        let poll = self.harnesses.poll();
        if poll.agents.is_empty() {
            return;
        }
        let current = boot.borrow().clone();
        let offer = match authority.harness_updates(&current, &poll).await {
            Ok(offer) => offer,
            // Older servers have no route; keep polling quietly.
            Err(error) => {
                tracing::debug!(%error, "harness_update_poll_failed");
                return;
            }
        };
        for directive in offer.directives {
            if cancel.is_cancelled() {
                return;
            }
            self.handle(activity, cancel, directive).await;
        }
        self.attest(authority, boot).await;
    }

    pub async fn adopt(
        &self,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
    ) {
        if let Err(error) = self.harnesses.adopt_staged() {
            tracing::warn!(%error, "harness_update_adoption_failed");
        }
        self.attest(authority, boot).await;
    }

    async fn handle(
        &self,
        activity: &dyn Activity,
        cancel: &CancellationToken,
        directive: HarnessDirective,
    ) {
        let artifact = &directive.artifact;
        let bundle = &artifact.bundle;
        if self.harnesses.handled(&directive.update_id, &artifact.url) {
            return;
        }
        let config = self.harnesses.config();
        // Trust and compatibility before any byte is downloaded.
        if let Err(error) = verify_bundle(
            bundle,
            &artifact.signature,
            &config.verifying_key,
            &config.architecture,
        )
        .and_then(|()| self.harnesses.compatible(bundle))
        {
            let code = store::code_of(&error);
            let url = (code == ARTIFACT_INVALID).then_some(artifact.url.as_str());
            self.harnesses.fail(&directive.update_id, bundle, code, url);
            return;
        }
        let file = BundleFile {
            bundle: bundle.clone(),
            signature: artifact.signature.clone(),
            update_id: directive.update_id.clone(),
        };
        // Already installed (a rollback to the previous bundle): stage only.
        if self.harnesses.installed(&bundle.sha256) {
            let tree = self.incoming().ok();
            let staged = match tree {
                Some(tree) => self.harnesses.stage(&tree, file),
                None => Err(anyhow::anyhow!(super::INSTALL_FAILED)),
            };
            if let Err(error) = staged {
                self.harnesses
                    .fail(&directive.update_id, bundle, store::code_of(&error), None);
            }
            return;
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = self.background_gate(activity) => {}
        }
        if artifact.url_expires_at <= crate::now_ms() + 5_000 {
            // The next poll brings a fresh URL.
            return;
        }
        let result = tokio::select! {
            _ = cancel.cancelled() => return,
            result = self.download(&directive) => result,
        };
        match result {
            Ok(()) => {
                tracing::info!(agent = %bundle.agent, version = %bundle.version, "Harness bundle staged")
            }
            Err(code) => {
                let url = (code != DOWNLOAD_FAILED).then_some(artifact.url.as_str());
                if code == DOWNLOAD_FAILED {
                    // Transient: the next poll offers the same update again.
                    tracing::warn!(update = %directive.update_id, "harness_update_download_failed");
                } else {
                    self.harnesses.fail(&directive.update_id, bundle, code, url);
                }
            }
        }
    }

    /// Never during the first turn after start or wake: wait until a turn
    /// finished, or the computer stayed idle for the grace period.
    async fn background_gate(&self, activity: &dyn Activity) {
        let mut saw_turn = false;
        loop {
            let since = super::last_wake()
                .filter(|w| *w > self.started)
                .unwrap_or(self.started)
                .elapsed();
            if activity.turn_active() {
                saw_turn = true;
            } else if saw_turn || since >= self.first_turn_grace {
                return;
            }
            tokio::time::sleep(BOUNDARY_CHECK).await;
        }
    }

    fn incoming(&self) -> anyhow::Result<PathBuf> {
        let root = self.harnesses.config().root.join("incoming");
        super::private_directory(&root)?;
        let dir = root.join(uuid::Uuid::new_v4().to_string());
        super::private_directory(&dir)?;
        Ok(dir)
    }

    /// Download, decompress, verify, extract and probe, then stage. Errors are codes.
    pub async fn download(&self, directive: &HarnessDirective) -> Result<(), &'static str> {
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
        let dir = self.incoming().map_err(|_| DOWNLOAD_FAILED)?;
        let result = async {
            let compressed = dir.join("bundle.tar.zst");
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
            // A dedicated low-priority thread, so verification never competes
            // with a turn and never lowers a shared blocking-pool thread.
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let (harnesses, dir2) = (self.harnesses.clone(), dir.clone());
            let file = BundleFile {
                bundle: artifact.bundle.clone(),
                signature: artifact.signature.clone(),
                update_id: directive.update_id.clone(),
            };
            std::thread::Builder::new()
                .name("harness-update".into())
                .spawn(move || {
                    lower_priority();
                    let _ = sender.send(install(&harnesses, &dir2, &compressed, file));
                })
                .map_err(|_| DOWNLOAD_FAILED)?;
            receiver
                .await
                .map_err(|_| DOWNLOAD_FAILED)?
                .map_err(|error| store::code_of(&error))
        }
        .await;
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// POST each pending attestation once; keep undelivered ones for later.
    pub async fn attest(
        &self,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
    ) {
        for attestation in self.harnesses.pending_attestations() {
            let current = boot.borrow().clone();
            match authority
                .harness_update_attestation(&current, &attestation)
                .await
            {
                Ok(()) => self.harnesses.attested(&attestation),
                Err(error) if crate::supervisor::retryable(&error) => {
                    tracing::debug!(%error, "harness_update_attestation_deferred");
                    return;
                }
                Err(error) => {
                    // A definitive refusal (for example a stale update) is dropped.
                    tracing::warn!(%error, "harness_update_attestation_refused");
                    self.harnesses.attested(&attestation);
                }
            }
        }
    }
}

/// Blocking: zstd → sha256 → extract → probe → stage.
fn install(
    harnesses: &Harnesses,
    dir: &std::path::Path,
    compressed: &std::path::Path,
    file: BundleFile,
) -> anyhow::Result<()> {
    let tar = dir.join("bundle.tar");
    store::decompress(compressed, &tar, &file.bundle.sha256)?;
    let _ = std::fs::remove_file(compressed);
    let tree = dir.join("tree");
    super::private_directory(&tree)?;
    let executable = store::extract(&tar, &tree, &file.bundle)?;
    let _ = std::fs::remove_file(&tar);
    store::probe(&executable, &file.bundle.version)?;
    harnesses.stage(&tree, file)
}

fn lower_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        // Raising one's own nice value is always permitted; this thread only.
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}
