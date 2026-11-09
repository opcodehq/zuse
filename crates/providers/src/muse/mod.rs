//! Native Muse Code driver: `muse serve` over the Muse Session Protocol
//! (newline-delimited JSON-RPC 2.0 on stdio, schema version 1).
//!
//! One `muse serve` host per runtime. The session opens with the `:ask-me`
//! permission profile and `promptUnmatched` approvals, so every tool Muse's
//! own rules do not already allow comes back as an `approval/requested`
//! event that Zuse settles — choosing only among the choices Muse offered.
//! The Muse session id is the resume cursor.
mod catalog;
mod normalize;
mod rpc;
mod session;

use crate::{Harness, HarnessError, RunControls};
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;
use zuse_protocol::{AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode};

/// The first release whose session protocol this driver speaks.
pub const MINIMUM_VERSION: semver::Version = semver::Version::new(1, 4, 6);

pub struct MuseHarness {
    executable: Option<PathBuf>,
    models_cache: crate::catalog::Catalog,
    init_timeout: Duration,
    interrupt_grace: Duration,
    kill_grace: Duration,
}

impl Default for MuseHarness {
    fn default() -> Self {
        Self {
            executable: None,
            models_cache: Default::default(),
            init_timeout: Duration::from_secs(15),
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(3),
        }
    }
}

impl MuseHarness {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }
    /// How long `initialize` may take before the host counts as unresponsive.
    pub fn with_init_timeout(mut self, timeout: Duration) -> Self {
        self.init_timeout = timeout;
        self
    }
    pub fn with_graces(mut self, interrupt: Duration, kill: Duration) -> Self {
        self.interrupt_grace = interrupt;
        self.kill_grace = kill;
        self
    }

    /// `MUSE_EXECUTABLE` (or an explicit path) wins; otherwise PATH and the
    /// installer's default locations.
    pub fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(path) = self
            .executable
            .clone()
            .or_else(|| std::env::var_os("MUSE_EXECUTABLE").map(PathBuf::from))
        {
            return crate::executable::validate_native_override(&path);
        }
        crate::executable::find_on_paths("muse", default_locations()).ok_or_else(|| {
            HarnessError::NotInstalled(
                "Muse Code CLI: install it with `curl -fsSL https://dev.meta.ai/install.sh | bash` or set MUSE_EXECUTABLE".into(),
            )
        })
    }

    /// Resolve and refuse releases older than the protocol this driver speaks.
    /// An explicit test/override path is trusted as-is.
    fn launchable(&self) -> Result<PathBuf, HarnessError> {
        let exe = self.resolve_executable()?;
        if self.executable.is_none()
            && let Some(version) = crate::executable::binary_version(&exe)
            && version < MINIMUM_VERSION
        {
            return Err(HarnessError::Protocol(format!(
                "Muse Code {version} is unsupported; update Muse Code to {MINIMUM_VERSION} or newer"
            )));
        }
        Ok(exe)
    }

    async fn discover_models(&self) -> Result<Vec<Model>, HarnessError> {
        let exe = self.launchable()?;
        let cwd = std::env::current_dir()?;
        let mut host = rpc::Host::open(
            &exe,
            &cwd,
            rpc::Mode::Probe,
            self.init_timeout,
            self.kill_grace,
        )
        .await?;
        let result = host.client.request("model/list", json!({})).await;
        host.child.shutdown(self.kill_grace).await;
        Ok(catalog::models(&result?))
    }
}

/// Where Muse Code's installer puts the CLI.
fn default_locations() -> Vec<PathBuf> {
    let home = crate::executable::home_or_current_dir();
    let mut paths = vec![home.join(".local/bin/muse")];
    if cfg!(windows) {
        let local = crate::model_context::root("LOCALAPPDATA", home.join("AppData/Local"));
        paths.push(local.join("Programs").join("muse").join("muse.cmd"));
        paths.push(local.join("Programs").join("muse").join("muse.exe"));
    }
    paths
}

#[async_trait]
impl Harness for MuseHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Muse
    }
    fn display_name(&self) -> &str {
        "Muse Code"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// `turn/steer` injects into the running turn; a steer that misses it
    /// becomes the next turn.
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    /// Efforts are per model (`model/list` variants).
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    /// Every turn ends with Muse's own durable `turn/completed`.
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    /// `turn/interrupt` ends the turn; the host and its background work stay.
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    /// Effort rides each `turn/start`, and `session/setModel` switches the
    /// model of the live session. Clearing a model back to the default
    /// cannot be expressed in place.
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && live.sandbox == next.sandbox
            && live.plan == next.plan
            && live.auto_approve == next.auto_approve
            && (next.model.is_some() || live.model.is_none())
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        crate::model_context::context(HarnessId::Muse, &self.resolve_executable()?, &[]).map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with(
                force,
                || Ok(self.model_context()?.expect("Muse model context").key()),
                || self.discover_models(),
            )
            .await
    }
    async fn commands(&self) -> Result<Vec<zuse_protocol::SlashCommand>, HarnessError> {
        Ok(vec![zuse_protocol::SlashCommand {
            name: "compact".into(),
            description: "Compact this conversation's context".into(),
            input_hint: None,
        }])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.launchable()?;
        if session::is_compact(&request.prompt) && request.resume.is_none() {
            return Err(HarnessError::Protocol(
                "/compact needs an existing Muse Code conversation".into(),
            ));
        }
        let (tx, rx) = mpsc::channel(256);
        let config = session::Config {
            exe,
            init_timeout: self.init_timeout,
            interrupt_grace: self.interrupt_grace,
            kill_grace: self.kill_grace,
        };
        tokio::spawn(session::run(config, request, controls, tx));
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
                .boxed(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> RunRequest {
        serde_json::from_value(json!({"prompt":"hi","cwd":"/tmp","sandbox":"workspace-write"}))
            .unwrap()
    }

    #[test]
    fn model_changes_apply_in_place_but_access_changes_do_not() {
        let harness = MuseHarness::new();
        let live = request();
        let mut next = request();
        next.model = Some("muse-test".into());
        next.reasoning = Some(ReasoningLevel::High);
        assert!(harness.reconfigures_in_place(&live, &next));
        assert!(!harness.reconfigures_in_place(&next, &live));
        next.plan = true;
        assert!(!harness.reconfigures_in_place(&live, &next));
    }

    #[test]
    fn installer_locations_include_the_user_bin() {
        assert!(
            default_locations()
                .iter()
                .any(|p| p.ends_with(".local/bin/muse"))
        );
    }
}
