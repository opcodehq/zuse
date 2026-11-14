//! Boat user image builder: builder sandbox from the managed base's saved
//! snapshot → setup script → sanitize → saved snapshot → verifier sandbox
//! from that snapshot → publish. Same phases and guest helper as Boxd.
//!
//! Boat specifics:
//! - An image is a *saved snapshot*, addressed by name. Its `snapshotId` is
//!   the version: saving a name again gets a new one, so the identity pins the
//!   exact disk.
//! - Saving a running sandbox captures its disk in the background and may
//!   include later writes, so nothing touches the builder after the save
//!   request, and the builder is deleted only once the save is `ready`.
//! - Disks restore lazily. A builder or verifier counts as up only once Boat
//!   reports it hydrated, matching how Boat bases are started (after
//!   hydration, with the runtime unit disabled in the image).
//! - Sandboxes are found by their unique name (a listing), since the API
//!   looks sandboxes up by ID only.
use super::boxd::quote;
use super::*;
use crate::managed_bases::bootstrap::{Purpose, Request as BootRequest};
use crate::providers::{
    ExecResult, MachineState, ProviderError,
    boat::{Boat, Sandbox, SavedSnapshot, SavedSnapshotState},
    contract::Provider,
};
use serde_json::json;
use std::sync::Arc;

/// Sandbox lifetime for a verifier, and the margin a builder gets beyond the
/// setup script's own limit. Boat archives (does not delete) at expiry.
const VERIFIER_TTL_SECONDS: u32 = 1800;
const BUILDER_MARGIN_SECONDS: u32 = 1800;
const JOURNAL: &str = "/opt/zuse/user-image";

/// The Boat calls the builder needs, so the phase logic is testable without a
/// provider account.
#[async_trait::async_trait]
pub trait BoatImageHost: Send + Sync {
    async fn sandbox(&self, id: &str) -> Result<Option<Sandbox>, ProviderError>;
    async fn find_sandbox(&self, name: &str) -> Result<Option<Sandbox>, ProviderError>;
    async fn create_from_snapshot(
        &self,
        snapshot: &str,
        name: &str,
        lifetime_seconds: u32,
    ) -> Result<Sandbox, ProviderError>;
    async fn exec(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError>;
    async fn destroy(&self, id: &str) -> Result<(), ProviderError>;
    async fn save_snapshot(&self, id: &str, name: &str) -> Result<(), ProviderError>;
    async fn snapshot(&self, name: &str) -> Result<Option<SavedSnapshot>, ProviderError>;
    async fn delete_snapshot(&self, name: &str) -> Result<(), ProviderError>;
}

#[async_trait::async_trait]
impl BoatImageHost for Boat {
    async fn sandbox(&self, id: &str) -> Result<Option<Sandbox>, ProviderError> {
        Boat::sandbox(self, id).await
    }
    async fn find_sandbox(&self, name: &str) -> Result<Option<Sandbox>, ProviderError> {
        Boat::find_sandbox(self, name).await
    }
    async fn create_from_snapshot(
        &self,
        snapshot: &str,
        name: &str,
        lifetime_seconds: u32,
    ) -> Result<Sandbox, ProviderError> {
        self.create_from_saved_snapshot(snapshot, name, lifetime_seconds)
            .await
    }
    async fn exec(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        self.execute(id, command).await
    }
    async fn destroy(&self, id: &str) -> Result<(), ProviderError> {
        self.delete(id).await.map(|_| ())
    }
    async fn save_snapshot(&self, id: &str, name: &str) -> Result<(), ProviderError> {
        let saved = self.save_prepared_snapshot(id, name).await?;
        if saved != name {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(())
    }
    async fn snapshot(&self, name: &str) -> Result<Option<SavedSnapshot>, ProviderError> {
        self.saved_snapshot(name).await
    }
    async fn delete_snapshot(&self, name: &str) -> Result<(), ProviderError> {
        self.delete_saved_snapshot(name).await
    }
}

pub struct BoatUserImages {
    host: Arc<dyn BoatImageHost>,
}

/// Map a provider failure to a retryable outcome, logging which call failed and
/// how (variant only; provider errors carry no bodies or credentials).
#[track_caller]
fn uncertain_here() -> impl FnOnce(ProviderError) -> ApiError {
    let at = std::panic::Location::caller();
    move |error| {
        tracing::warn!(line = at.line(), error = ?error, provider = "boat", "User image provider call failed");
        ApiError::unavailable("user_image_provider_outcome_uncertain")
    }
}

impl BoatUserImages {
    pub fn new(host: Arc<dyn BoatImageHost>) -> Self {
        Self { host }
    }
    /// Write a file as root in exec-sized chunks; decoding runs in the guest's
    /// python3. Provider file uploads are not used (they proved unreliable on
    /// real builders).
    async fn write_root_file(&self, id: &str, path: &str, data: &[u8]) -> Result<(), ApiError> {
        use base64::Engine as _;
        const WRITE: &str = "import base64,sys\np,m,d=sys.argv[1:4]\nwith open(p,m+'b') as f: f.write(base64.b64decode(d))";
        let chunks: Vec<&[u8]> = if data.is_empty() {
            vec![&[]]
        } else {
            data.chunks(30_000).collect()
        };
        for (n, chunk) in chunks.into_iter().enumerate() {
            let command = format!(
                "sudo -n /usr/bin/python3 -I -B -c {} {} {} {}",
                quote(WRITE),
                quote(path),
                if n == 0 { "w" } else { "a" },
                quote(&base64::engine::general_purpose::STANDARD.encode(chunk))
            );
            let result = self
                .host
                .exec(id, &command)
                .await
                .map_err(uncertain_here())?;
            if result.exit_code != 0 {
                return Err(ApiError::conflict("user_image_upload_failed"));
            }
        }
        Ok(())
    }
    /// Run the guest helper as root, inline: the copy on the image is never
    /// trusted for a check, since the setup script could have changed it.
    async fn helper(&self, id: &str, args: &[&str]) -> Result<Value, ApiError> {
        let command = format!(
            "sudo -n /usr/bin/python3 -I -B -c {} {}",
            quote(GUEST_HELPER),
            args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
        );
        let result = self
            .host
            .exec(id, &command)
            .await
            .map_err(uncertain_here())?;
        if result.stdout.len() > 256 * 1024 {
            return Err(ApiError::conflict("user_image_guest_receipt_invalid"));
        }
        let value: Value = serde_json::from_slice(&result.stdout)
            .map_err(|_| ApiError::conflict("user_image_guest_receipt_invalid"))?;
        if value["version"] != 1 {
            return Err(ApiError::conflict("user_image_guest_receipt_invalid"));
        }
        Ok(value)
    }
    fn bootstrap(build: &Build) -> Result<&InitialRuntimeBootstrap, ApiError> {
        build
            .plan
            .initial_runtime_bootstrap
            .as_ref()
            .ok_or_else(|| ApiError::conflict("user_image_plan_invalid"))
    }
    fn pins(build: &Build) -> Result<String, ApiError> {
        serde_json::to_string(&Self::bootstrap(build)?.pins())
            .map_err(|_| ApiError::conflict("user_image_plan_invalid"))
    }
    /// The recorded sandbox, else the one with this name.
    async fn lookup(&self, id: Option<&str>, name: &str) -> Result<Option<Sandbox>, ApiError> {
        let found = match id {
            Some(id) => self.host.sandbox(id).await,
            None => self.host.find_sandbox(name).await,
        }
        .map_err(uncertain_here())?;
        if let Some(sandbox) = &found
            && (sandbox.machine.name != name || id.is_some_and(|id| id != sandbox.machine.id))
        {
            return Err(ApiError::conflict("user_image_resource_identity_changed"));
        }
        Ok(found)
    }
    /// Find the sandbox with this exact name, creating it only when none has
    /// that name and none was recorded. `Ok(Err(()))`: a recorded sandbox
    /// disappeared.
    async fn machine(
        &self,
        resources: &mut Value,
        field: &str,
        name: &str,
        snapshot: &str,
        lifetime_seconds: u32,
    ) -> Result<Result<Sandbox, ()>, ApiError> {
        let sandbox = match self.lookup(resources[field].as_str(), name).await? {
            Some(sandbox) => sandbox,
            None if resources[field].is_null() => self
                .host
                .create_from_snapshot(snapshot, name, lifetime_seconds)
                .await
                .map_err(uncertain_here())?,
            None => return Ok(Err(())),
        };
        if sandbox.machine.name != name || sandbox.source_artifact.as_deref() != Some(snapshot) {
            return Err(ApiError::conflict("user_image_resource_identity_changed"));
        }
        resources[field] = json!(sandbox.machine.id);
        Ok(Ok(sandbox))
    }
    /// A saved snapshot that is ready and still the exact recorded disk.
    async fn ready_snapshot(&self, name: &str, version: &str) -> Result<bool, ApiError> {
        let snapshot = self.host.snapshot(name).await.map_err(uncertain_here())?;
        Ok(snapshot.is_some_and(|s| {
            s.name == name
                && s.state == SavedSnapshotState::Ready
                && s.version.as_deref() == Some(version)
        }))
    }
    async fn retire_machine(&self, id: Option<&str>, name: &str) -> Result<bool, ApiError> {
        let Some(sandbox) = self.lookup(id, name).await? else {
            return Ok(true);
        };
        self.host
            .destroy(&sandbox.machine.id)
            .await
            .map_err(uncertain_here())?;
        Ok(self
            .host
            .sandbox(&sandbox.machine.id)
            .await
            .map_err(uncertain_here())?
            .is_none())
    }
    fn failed(code: &'static str, log: Option<String>, resources: Value) -> Result<Step, ApiError> {
        Ok(Step::Failed {
            code,
            log,
            resources,
        })
    }
    fn up(sandbox: &Sandbox) -> bool {
        sandbox.machine.state == MachineState::Running && sandbox.hydrated
    }
}

#[async_trait::async_trait]
impl UserImageBackend for BoatUserImages {
    async fn base_version(
        &self,
        provider: ProviderId,
        artifact: &str,
    ) -> Result<Option<String>, ApiError> {
        if provider != ProviderId::Boat {
            return Ok(None);
        }
        let snapshot = self
            .host
            .snapshot(artifact)
            .await
            .map_err(uncertain_here())?;
        Ok(snapshot
            .filter(|s| s.name == artifact && s.state == SavedSnapshotState::Ready)
            .and_then(|s| s.version)
            .filter(|v| !v.is_empty()))
    }

    async fn step(&self, build: &Build, mode: StepMode) -> Result<Step, ApiError> {
        let plan = &build.plan;
        if plan.identity.provider != ProviderId::Boat {
            return Err(ApiError::conflict("user_image_provider_not_supported"));
        }
        let (builder_name, verifier_name, snapshot_name) = plan.names();
        let mut resources = build.resources["provider"]
            .as_object()
            .cloned()
            .map(Value::Object)
            .unwrap_or_else(|| json!({}));
        let base = plan.identity.base_artifact_id.clone();
        let advance = |phase: &str, resources: Value| {
            Ok(Step::Advance {
                phase: phase.into(),
                resources,
            })
        };
        let builder_id = |resources: &Value| {
            resources["builderId"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| ApiError::conflict("user_image_builder_missing"))
        };
        match build.phase.as_str() {
            "allocate" => {
                if resources["builderId"].is_null()
                    && !self
                        .ready_snapshot(&base, &plan.identity.base_artifact_version)
                        .await?
                {
                    // The base was re-saved or removed since the plan froze.
                    resources["builderNotSubmitted"] = json!(true);
                    return Self::failed("user_image_managed_base_changed", None, resources);
                }
                let lifetime = (plan.setup_timeout_seconds + BUILDER_MARGIN_SECONDS).min(86400);
                let Ok(sandbox) = self
                    .machine(&mut resources, "builderId", &builder_name, &base, lifetime)
                    .await?
                else {
                    return Self::failed("user_image_builder_lost", None, resources);
                };
                if !Self::up(&sandbox) {
                    return Ok(Step::Pending(resources));
                }
                let pins = self
                    .helper(&sandbox.machine.id, &["pins", &Self::pins(build)?])
                    .await?;
                if pins["pinsVerified"] != true {
                    return Self::failed("user_image_managed_base_pins_mismatch", None, resources);
                }
                resources["pinsVerified"] = json!(true);
                advance("setup", resources)
            }
            "setup" => {
                let id = builder_id(&resources)?;
                let status = self.helper(&id, &["setup-status"]).await?;
                match status["state"].as_str() {
                    Some("exited") => {
                        let log = status["log"].as_str().map(str::to_owned);
                        return match status["exitCode"].as_i64() {
                            Some(0) => advance("sanitize", resources),
                            Some(124 | 137) => {
                                Self::failed("user_image_setup_timeout", log, resources)
                            }
                            _ => Self::failed("user_image_setup_failed", log, resources),
                        };
                    }
                    Some("running") => return Ok(Step::Pending(resources)),
                    Some("lost") => {
                        let log = status["log"].as_str().map(str::to_owned);
                        return Self::failed("user_image_setup_interrupted", log, resources);
                    }
                    Some("not-started") => {}
                    _ => return Err(ApiError::conflict("user_image_guest_receipt_invalid")),
                }
                if mode == StepMode::Reconcile && resources["setupStarted"] == true {
                    return Self::failed("user_image_setup_interrupted", None, resources);
                }
                let mkdir = self
                    .host
                    .exec(
                        &id,
                        &format!("sudo -n install -d -m 700 -o root -g root {JOURNAL}"),
                    )
                    .await
                    .map_err(uncertain_here())?;
                if mkdir.exit_code != 0 {
                    return Err(ApiError::conflict("user_image_upload_failed"));
                }
                self.write_root_file(
                    &id,
                    &format!("{JOURNAL}/setup.sh"),
                    plan.setup_script.as_bytes(),
                )
                .await?;
                self.write_root_file(&id, &format!("{JOURNAL}/guest.py"), GUEST_HELPER.as_bytes())
                    .await?;
                let fix = self
                    .host
                    .exec(&id, &format!("sudo -n sh -c {}", quote(&format!("chown root:root {JOURNAL} {JOURNAL}/setup.sh {JOURNAL}/guest.py && chmod 644 {JOURNAL}/setup.sh && chmod 600 {JOURNAL}/guest.py && chmod 755 {JOURNAL}"))))
                    .await
                    .map_err(uncertain_here())?;
                if fix.exit_code != 0 {
                    return Err(ApiError::conflict("user_image_upload_failed"));
                }
                resources["setupStarted"] = json!(true);
                let started = self
                    .helper(
                        &id,
                        &["setup-start", &plan.setup_timeout_seconds.to_string()],
                    )
                    .await?;
                if started["state"] != "running" && started["state"] != "exited" {
                    return Err(ApiError::conflict("user_image_guest_receipt_invalid"));
                }
                Ok(Step::Pending(resources))
            }
            "sanitize" => {
                let id = builder_id(&resources)?;
                let pins = Self::pins(build)?;
                let result = self.helper(&id, &["sanitize", &pins]).await?;
                if result["sanitized"] == true
                    && result["pinsVerified"] == true
                    && result["runtimeStopped"] == true
                {
                    resources["sanitized"] = json!(true);
                    return advance("capture", resources);
                }
                let check = self.helper(&id, &["check", &pins]).await?;
                if check["clean"] == true && mode == StepMode::Reconcile {
                    resources["sanitized"] = json!(true);
                    return advance("capture", resources);
                }
                Self::failed(
                    match result["error"].as_str() {
                        Some(
                            "user_image_pinned_file_changed"
                            | "user_image_pinned_file_unsafe"
                            | "user_image_path_symlink",
                        ) => "user_image_runtime_files_changed",
                        Some("user_image_runtime_started" | "user_image_runtime_state_present") => {
                            "user_image_runtime_started"
                        }
                        Some("user_image_processes_running") => "user_image_processes_running",
                        _ => "user_image_sanitize_failed",
                    },
                    None,
                    resources,
                )
            }
            "capture" => {
                let id = builder_id(&resources)?;
                let snapshot = self
                    .host
                    .snapshot(&snapshot_name)
                    .await
                    .map_err(uncertain_here())?;
                let snapshot = match snapshot {
                    Some(s) => s,
                    None if resources["artifactId"].is_null() => {
                        if resources["sanitized"] != true {
                            resources["captureNotSubmitted"] = json!(true);
                            return Self::failed("user_image_sanitize_failed", None, resources);
                        }
                        // A lost reply is found by name next tick. Saving the
                        // same name again only replaces the disk with the same
                        // untouched, sanitized builder disk.
                        self.host
                            .save_snapshot(&id, &snapshot_name)
                            .await
                            .map_err(uncertain_here())?;
                        resources["artifactId"] = json!(snapshot_name);
                        return Ok(Step::Pending(resources));
                    }
                    None => return Self::failed("user_image_capture_failed", None, resources),
                };
                if snapshot.name != snapshot_name
                    || snapshot
                        .source_machine_id
                        .as_deref()
                        .is_some_and(|s| s != id)
                    || snapshot_name == plan.identity.base_artifact_id
                {
                    return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                }
                resources["artifactId"] = json!(snapshot.name);
                match snapshot.state {
                    SavedSnapshotState::Failed => {
                        Self::failed("user_image_capture_failed", None, resources)
                    }
                    SavedSnapshotState::Saving => Ok(Step::Pending(resources)),
                    SavedSnapshotState::Ready => {
                        let Some(version) = snapshot.version.filter(|v| !v.is_empty()) else {
                            return Err(ApiError::unavailable(
                                "user_image_artifact_version_missing",
                            ));
                        };
                        resources["artifactVersion"] = json!(version);
                        advance("retire-builder", resources)
                    }
                }
            }
            "retire-builder" => {
                if !self
                    .retire_machine(resources["builderId"].as_str(), &builder_name)
                    .await?
                {
                    return Ok(Step::Pending(resources));
                }
                resources["builderAbsent"] = json!(true);
                advance("verify", resources)
            }
            "verify" => {
                let artifact = resources["artifactId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_artifact_missing"))?
                    .to_owned();
                let version = resources["artifactVersion"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_artifact_missing"))?
                    .to_owned();
                if resources["verifierId"].is_null()
                    && !self.ready_snapshot(&artifact, &version).await?
                {
                    resources["verifierNotSubmitted"] = json!(true);
                    return Self::failed("user_image_artifact_missing", None, resources);
                }
                let Ok(sandbox) = self
                    .machine(
                        &mut resources,
                        "verifierId",
                        &verifier_name,
                        &artifact,
                        VERIFIER_TTL_SECONDS,
                    )
                    .await?
                else {
                    return Self::failed("user_image_verifier_lost", None, resources);
                };
                if !Self::up(&sandbox) {
                    return Ok(Step::Pending(resources));
                }
                let id = sandbox.machine.id.clone();
                let pins = Self::pins(build)?;
                // Clean before the resident starts: nothing leaked into the image.
                let check = self.helper(&id, &["check", &pins]).await?;
                if check["clean"] != true {
                    return Self::failed("user_image_verification_failed", None, resources);
                }
                // The same post-hydration first start a task computer gets: the
                // base's pins and an unbound resident started from this image.
                let request = BootRequest {
                    version: 1,
                    provider: ProviderId::Boat,
                    purpose: Purpose::CachePreparation,
                    operation_id: build.image_id.clone(),
                    workspace_id: format!("user-image:{}", plan.identity.key()?),
                    generation: 1,
                    storage_incarnation_id: format!("user-image:{}", build.image_id),
                    source_machine_id: id.clone(),
                    image_pins: Self::bootstrap(build)?.pins(),
                };
                let result = self
                    .host
                    .exec(&id, &request.command().map_err(uncertain_here())?)
                    .await
                    .map_err(uncertain_here())?;
                if result.exit_code != 0 || result.stdout.len() > 65536 {
                    return Self::failed("user_image_fresh_boot_rejected", None, resources);
                }
                let value: Value = serde_json::from_slice(&result.stdout)
                    .map_err(|_| ApiError::conflict("user_image_guest_receipt_invalid"))?;
                match request.response(&value) {
                    Ok(Some(_)) => advance("retire-verifier", resources),
                    Ok(None) => Ok(Step::Pending(resources)),
                    Err(_) => Self::failed("user_image_fresh_boot_rejected", None, resources),
                }
            }
            "retire-verifier" => {
                if !self
                    .retire_machine(resources["verifierId"].as_str(), &verifier_name)
                    .await?
                {
                    return Ok(Step::Pending(resources));
                }
                resources["verifierAbsent"] = json!(true);
                advance("publish", resources)
            }
            "publish" => {
                let artifact = resources["artifactId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_artifact_missing"))?
                    .to_owned();
                let version = resources["artifactVersion"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_artifact_missing"))?
                    .to_owned();
                if artifact != snapshot_name || !self.ready_snapshot(&artifact, &version).await? {
                    return Self::failed("user_image_artifact_missing", None, resources);
                }
                Ok(Step::Ready {
                    artifact_id: artifact,
                    artifact_version: Some(version),
                    resources,
                })
            }
            "cleanup" => {
                for (field, name, effect, not_submitted) in [
                    (
                        "builderId",
                        &builder_name,
                        "allocate",
                        "builderNotSubmitted",
                    ),
                    (
                        "verifierId",
                        &verifier_name,
                        "verify",
                        "verifierNotSubmitted",
                    ),
                ] {
                    if resources[field].is_null() {
                        if let Some(sandbox) = self.lookup(None, name).await? {
                            resources[field] = json!(sandbox.machine.id);
                        } else if build.resources["effects"][effect]["requested"] == true
                            && resources[not_submitted] != true
                        {
                            resources[not_submitted] = json!(true);
                            return Ok(Step::Pending(resources));
                        }
                    }
                }
                if !self
                    .retire_machine(resources["builderId"].as_str(), &builder_name)
                    .await?
                    || !self
                        .retire_machine(resources["verifierId"].as_str(), &verifier_name)
                        .await?
                {
                    return Ok(Step::Pending(resources));
                }
                resources["builderAbsent"] = json!(true);
                resources["verifierAbsent"] = json!(true);
                if snapshot_name == plan.identity.base_artifact_id {
                    return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                }
                if let Some(snapshot) = self
                    .host
                    .snapshot(&snapshot_name)
                    .await
                    .map_err(uncertain_here())?
                {
                    if snapshot.name != snapshot_name {
                        return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                    }
                    resources["artifactId"] = json!(snapshot.name);
                    self.host
                        .delete_snapshot(&snapshot.name)
                        .await
                        .map_err(uncertain_here())?;
                    return Ok(Step::Pending(resources));
                }
                resources["unpublishedArtifactAbsent"] = json!(true);
                advance("complete", resources)
            }
            "retire" => {
                let artifact = build
                    .artifact_id
                    .clone()
                    .ok_or_else(|| ApiError::conflict("user_image_artifact_missing"))?;
                if artifact == plan.identity.base_artifact_id || artifact != snapshot_name {
                    return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                }
                if let Some(snapshot) = self
                    .host
                    .snapshot(&artifact)
                    .await
                    .map_err(uncertain_here())?
                {
                    if snapshot.name != snapshot_name {
                        return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                    }
                    self.host
                        .delete_snapshot(&snapshot.name)
                        .await
                        .map_err(uncertain_here())?;
                    return Ok(Step::Pending(resources));
                }
                resources["artifactAbsent"] = json!(true);
                advance("retired", resources)
            }
            _ => Err(ApiError::conflict("user_image_phase_invalid")),
        }
    }
}
