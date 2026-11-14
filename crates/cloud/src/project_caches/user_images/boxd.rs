//! Boxd user image builder: builder VM from the managed base snapshot → setup
//! script → sanitize → snapshot → verifier VM from that snapshot → publish.
//! Every resource has a name derived from the image ID, so a lost reply is
//! answered by inspecting that exact resource; nothing is created twice.
use super::*;
use crate::managed_bases::bootstrap::{Purpose, Request as BootRequest};
use crate::providers::{
    ExecResult, Machine, MachineState, ProviderError,
    boxd::{Boxd, Snapshot, SnapshotCapture, SnapshotState},
};
use serde_json::json;
use std::sync::Arc;

/// The provider calls the builder needs, so the phase logic is testable
/// without a provider account.
#[async_trait::async_trait]
pub trait ImageHost: Send + Sync {
    async fn inspect(&self, reference: &str) -> Result<Option<Machine>, ProviderError>;
    async fn create_from_snapshot(
        &self,
        snapshot: &str,
        name: &str,
    ) -> Result<Machine, ProviderError>;
    async fn verify_snapshot_source(
        &self,
        machine: &str,
        snapshot: &str,
        version: u64,
    ) -> Result<(), ProviderError>;
    async fn verify_initial_snapshot_source(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
    ) -> Result<(), ProviderError>;
    async fn exec(&self, machine: &str, command: &str) -> Result<ExecResult, ProviderError>;
    async fn upload(&self, machine: &str, path: &str, data: &[u8]) -> Result<(), ProviderError>;
    async fn destroy(&self, machine: &str) -> Result<(), ProviderError>;
    async fn create_snapshot(
        &self,
        machine: &str,
        name: &str,
    ) -> Result<SnapshotCapture, ProviderError>;
    async fn inspect_snapshot(&self, reference: &str) -> Result<Option<Snapshot>, ProviderError>;
    async fn delete_snapshot(&self, reference: &str) -> Result<(), ProviderError>;
}

#[async_trait::async_trait]
impl ImageHost for Boxd {
    async fn inspect(&self, reference: &str) -> Result<Option<Machine>, ProviderError> {
        Boxd::inspect(self, reference).await
    }
    async fn create_from_snapshot(
        &self,
        snapshot: &str,
        name: &str,
    ) -> Result<Machine, ProviderError> {
        Boxd::create_from_snapshot(self, snapshot, name, true).await
    }
    async fn verify_snapshot_source(
        &self,
        machine: &str,
        snapshot: &str,
        version: u64,
    ) -> Result<(), ProviderError> {
        Boxd::verify_snapshot_source(self, machine, snapshot, version).await
    }
    async fn verify_initial_snapshot_source(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
    ) -> Result<(), ProviderError> {
        Boxd::verify_initial_snapshot_source(self, machine, artifact, name).await
    }
    async fn exec(&self, machine: &str, command: &str) -> Result<ExecResult, ProviderError> {
        Boxd::exec(self, machine, command).await
    }
    async fn upload(&self, machine: &str, path: &str, data: &[u8]) -> Result<(), ProviderError> {
        Boxd::upload(self, machine, path, data).await
    }
    async fn destroy(&self, machine: &str) -> Result<(), ProviderError> {
        Boxd::destroy(self, machine).await
    }
    async fn create_snapshot(
        &self,
        machine: &str,
        name: &str,
    ) -> Result<SnapshotCapture, ProviderError> {
        Boxd::create_snapshot(self, machine, name).await
    }
    async fn inspect_snapshot(&self, reference: &str) -> Result<Option<Snapshot>, ProviderError> {
        Boxd::inspect_snapshot(self, reference).await
    }
    async fn delete_snapshot(&self, reference: &str) -> Result<(), ProviderError> {
        Boxd::delete_snapshot(self, reference).await
    }
}

pub struct BoxdUserImages {
    host: Arc<dyn ImageHost>,
}

/// Map a provider failure to a retryable outcome, logging which call failed and
/// how (variant only; provider errors carry no bodies or credentials).
#[track_caller]
fn uncertain_here() -> impl FnOnce(ProviderError) -> ApiError {
    let at = std::panic::Location::caller();
    move |error| {
        tracing::warn!(line = at.line(), error = ?error, "User image provider call failed");
        ApiError::unavailable("user_image_provider_outcome_uncertain")
    }
}
pub(crate) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
const JOURNAL: &str = "/opt/zuse/user-image";

impl BoxdUserImages {
    pub fn new(host: Arc<dyn ImageHost>) -> Self {
        Self { host }
    }
    /// Run the guest helper as root, inline: the copy on the image is never
    /// trusted for a check, since the setup script could have changed it.
    /// Write a file as root in exec-sized chunks (each command stays well under
    /// the provider's 64 KiB limit). Decoding runs in the guest's python3.
    async fn write_root_file(
        &self,
        machine: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), ApiError> {
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
                .exec(machine, &command)
                .await
                .map_err(uncertain_here())?;
            if result.exit_code != 0 {
                return Err(ApiError::conflict("user_image_upload_failed"));
            }
        }
        Ok(())
    }
    async fn helper(&self, machine: &str, args: &[&str]) -> Result<Value, ApiError> {
        let command = format!(
            "sudo -n /usr/bin/python3 -I -B -c {} {}",
            quote(GUEST_HELPER),
            args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
        );
        let result = self
            .host
            .exec(machine, &command)
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
    fn pins(build: &Build) -> Result<String, ApiError> {
        serde_json::to_string(&build.plan.initial_runtime_bootstrap.pins())
            .map_err(|_| ApiError::conflict("user_image_plan_invalid"))
    }
    /// Find the machine with this exact name, creating it only when no
    /// machine has that name and none was recorded. Names are unique per
    /// build, so a lost create reply is found here instead of duplicated.
    /// `Err(None)` means a recorded machine disappeared.
    async fn machine(
        &self,
        resources: &mut Value,
        field: &str,
        name: &str,
        snapshot: &str,
    ) -> Result<Result<Machine, ()>, ApiError> {
        let found = self
            .host
            .inspect(resources[field].as_str().unwrap_or(name))
            .await
            .map_err(uncertain_here())?;
        let machine = match found {
            Some(machine) => machine,
            None if resources[field].is_null() => self
                .host
                .create_from_snapshot(snapshot, name)
                .await
                .map_err(uncertain_here())?,
            None => return Ok(Err(())),
        };
        if machine.name != name || resources[field].as_str().is_some_and(|id| id != machine.id) {
            return Err(ApiError::conflict("user_image_resource_identity_changed"));
        }
        resources[field] = json!(machine.id);
        Ok(Ok(machine))
    }
    async fn retire_machine(&self, id: Option<&str>, name: &str) -> Result<bool, ApiError> {
        let Some(machine) = self
            .host
            .inspect(id.unwrap_or(name))
            .await
            .map_err(uncertain_here())?
        else {
            return Ok(true);
        };
        if machine.name != name || id.is_some_and(|id| id != machine.id) {
            return Err(ApiError::conflict("user_image_resource_identity_changed"));
        }
        self.host
            .destroy(&machine.id)
            .await
            .map_err(uncertain_here())?;
        Ok(self
            .host
            .inspect(&machine.id)
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
}

#[async_trait::async_trait]
impl UserImageBackend for BoxdUserImages {
    async fn base_version(
        &self,
        provider: ProviderId,
        artifact: &str,
    ) -> Result<Option<String>, ApiError> {
        if provider != ProviderId::Boxd {
            return Ok(None);
        }
        let snapshot = self
            .host
            .inspect_snapshot(artifact)
            .await
            .map_err(uncertain_here())?;
        Ok(snapshot
            .filter(|s| s.id == artifact && s.state == SnapshotState::Ready && s.version > 0)
            .map(|s| s.version.to_string()))
    }

    async fn step(&self, build: &Build, mode: StepMode) -> Result<Step, ApiError> {
        let plan = &build.plan;
        if plan.identity.provider != ProviderId::Boxd {
            return Err(ApiError::conflict("user_image_provider_not_supported"));
        }
        let (builder_name, verifier_name, snapshot_name) = plan.names();
        let mut resources = build.resources["provider"]
            .as_object()
            .cloned()
            .map(Value::Object)
            .unwrap_or_else(|| json!({}));
        let base_version: u64 = plan
            .identity
            .base_artifact_version
            .parse()
            .map_err(|_| ApiError::conflict("user_image_plan_invalid"))?;
        let advance = |phase: &str, resources: Value| {
            Ok(Step::Advance {
                phase: phase.into(),
                resources,
            })
        };
        match build.phase.as_str() {
            "allocate" => {
                let Ok(machine) = self
                    .machine(
                        &mut resources,
                        "builderId",
                        &builder_name,
                        &plan.identity.base_artifact_id,
                    )
                    .await?
                else {
                    return Self::failed("user_image_builder_lost", None, resources);
                };
                if machine.state != MachineState::Running {
                    return Ok(Step::Pending(resources));
                }
                self.host
                    .verify_snapshot_source(
                        &machine.id,
                        &plan.identity.base_artifact_id,
                        base_version,
                    )
                    .await
                    .map_err(uncertain_here())?;
                let pins = self
                    .helper(&machine.id, &["pins", &Self::pins(build)?])
                    .await?;
                if pins["pinsVerified"] != true {
                    // The base itself does not match its own registry pins.
                    return Self::failed("user_image_managed_base_pins_mismatch", None, resources);
                }
                resources["pinsVerified"] = json!(true);
                advance("setup", resources)
            }
            "setup" => {
                let id = resources["builderId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_builder_missing"))?
                    .to_owned();
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
                    // The launch was recorded but the guest has no trace of it.
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
                // Files are written as root through exec, not the provider's
                // upload call (which failed on real Boxd builders); the script
                // stays readable by `zuse` through sudo.
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
                let id = resources["builderId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_builder_missing"))?
                    .to_owned();
                let pins = Self::pins(build)?;
                let result = self.helper(&id, &["sanitize", &pins]).await?;
                if result["sanitized"] == true
                    && result["pinsVerified"] == true
                    && result["runtimeStopped"] == true
                {
                    resources["sanitized"] = json!(true);
                    return advance("capture", resources);
                }
                // A replayed sanitize after a lost reply finds the journal gone;
                // the read-only check decides whether the first run completed.
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
                let snapshot = self
                    .host
                    .inspect_snapshot(&snapshot_name)
                    .await
                    .map_err(uncertain_here())?;
                let snapshot = match snapshot {
                    Some(s) => s,
                    None if resources["artifactId"].is_null() => {
                        let id = resources["builderId"]
                            .as_str()
                            .ok_or_else(|| ApiError::conflict("user_image_builder_missing"))?
                            .to_owned();
                        if resources["sanitized"] != true {
                            resources["captureNotSubmitted"] = json!(true);
                            return Self::failed("user_image_sanitize_failed", None, resources);
                        }
                        let capture = self
                            .host
                            .create_snapshot(&id, &snapshot_name)
                            .await
                            .map_err(uncertain_here())?;
                        resources["artifactId"] = json!(capture.id);
                        resources["artifactVersion"] = json!(capture.version.to_string());
                        return Ok(Step::Pending(resources));
                    }
                    None => return Self::failed("user_image_capture_failed", None, resources),
                };
                if snapshot.name != snapshot_name
                    || resources["artifactId"]
                        .as_str()
                        .is_some_and(|id| id != snapshot.id)
                {
                    return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                }
                resources["artifactId"] = json!(snapshot.id);
                resources["artifactVersion"] = json!(snapshot.version.to_string());
                match snapshot.state {
                    SnapshotState::Failed => {
                        Self::failed("user_image_capture_failed", None, resources)
                    }
                    SnapshotState::Pending => Ok(Step::Pending(resources)),
                    SnapshotState::Ready => advance("retire-builder", resources),
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
                let version: u64 = resources["artifactVersion"]
                    .as_str()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| ApiError::conflict("user_image_artifact_missing"))?;
                let Ok(machine) = self
                    .machine(&mut resources, "verifierId", &verifier_name, &artifact)
                    .await?
                else {
                    return Self::failed("user_image_verifier_lost", None, resources);
                };
                if machine.state != MachineState::Running {
                    return Ok(Step::Pending(resources));
                }
                self.host
                    .verify_snapshot_source(&machine.id, &artifact, version)
                    .await
                    .map_err(uncertain_here())?;
                let pins = Self::pins(build)?;
                // Clean before the resident starts: nothing leaked into the image.
                let check = self.helper(&machine.id, &["check", &pins]).await?;
                if check["clean"] != true {
                    return Self::failed("user_image_verification_failed", None, resources);
                }
                // The same fresh boot a task computer gets: the base's pins and
                // an unbound resident starting from this snapshot.
                self.host
                    .verify_initial_snapshot_source(&machine.id, &artifact, &verifier_name)
                    .await
                    .map_err(uncertain_here())?;
                let request = BootRequest {
                    version: 1,
                    provider: ProviderId::Boxd,
                    purpose: Purpose::CachePreparation,
                    operation_id: build.image_id.clone(),
                    workspace_id: format!("user-image:{}", plan.identity.key()?),
                    generation: 1,
                    storage_incarnation_id: format!("user-image:{}", build.image_id),
                    source_machine_id: machine.id.clone(),
                    image_pins: plan.initial_runtime_bootstrap.pins(),
                };
                let result = self
                    .host
                    .exec(&machine.id, &request.command().map_err(uncertain_here())?)
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
                let snapshot = self
                    .host
                    .inspect_snapshot(&artifact)
                    .await
                    .map_err(uncertain_here())?;
                if snapshot.as_ref().is_none_or(|s| {
                    s.state != SnapshotState::Ready
                        || s.name != snapshot_name
                        || resources["artifactVersion"].as_str()
                            != Some(s.version.to_string().as_str())
                }) {
                    return Self::failed("user_image_artifact_missing", None, resources);
                }
                Ok(Step::Ready {
                    artifact_id: artifact,
                    artifact_version: resources["artifactVersion"].as_str().map(str::to_owned),
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
                        if let Some(machine) =
                            self.host.inspect(name).await.map_err(uncertain_here())?
                        {
                            if machine.name != *name {
                                return Err(ApiError::conflict(
                                    "user_image_resource_identity_changed",
                                ));
                            }
                            resources[field] = json!(machine.id);
                        } else if build.resources["effects"][effect]["requested"] == true
                            && resources[not_submitted] != true
                        {
                            // A create whose reply was lost may still surface:
                            // look once more on the next tick before trusting
                            // its absence.
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
                if let Some(snapshot) = self
                    .host
                    .inspect_snapshot(&snapshot_name)
                    .await
                    .map_err(uncertain_here())?
                {
                    if snapshot.name != snapshot_name
                        || snapshot.id == plan.identity.base_artifact_id
                    {
                        return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                    }
                    resources["artifactId"] = json!(snapshot.id);
                    self.host
                        .delete_snapshot(&snapshot.id)
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
                if artifact == plan.identity.base_artifact_id {
                    return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                }
                if let Some(snapshot) = self
                    .host
                    .inspect_snapshot(&artifact)
                    .await
                    .map_err(uncertain_here())?
                {
                    if snapshot.name != snapshot_name {
                        return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                    }
                    self.host
                        .delete_snapshot(&snapshot.id)
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
