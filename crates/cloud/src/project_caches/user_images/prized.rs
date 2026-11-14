//! Prized user image builder. Prized sandboxes have no disk snapshots or
//! custom images; the documented template is a paused sandbox that every job
//! forks (memory, disk and processes). The managed base is one, and so is a
//! user image: builder forked from the paused base → setup script → sanitize
//! (idle supervisor restarted) → pause, and the paused builder IS the image →
//! verifier forked from it → publish. Computers then fork the image exactly as
//! they fork the base. Every sandbox carries a name derived from the image ID,
//! so a lost reply is answered by finding that exact sandbox.
//!
//! A paused sandbox is never sent a command: any command wakes it.
use super::boxd::{quote, uncertain_here};
use super::*;
use crate::providers::{
    ExecResult, Machine, MachineState, ProviderError, contract::Provider, prized::Prized,
};
use serde_json::json;
use std::sync::Arc;

/// Builders run the setup script (up to an hour) plus sanitize; a verifier
/// only checks. A paused sandbox's clock stops, so the image never expires.
const BUILDER_RUNTIME_MIN: u32 = 120;
const VERIFIER_RUNTIME_MIN: u32 = 30;
const JOURNAL: &str = "/home/prized/.zuse-user-image";

/// The provider calls the builder needs, so the phase logic is testable
/// without a provider account.
#[async_trait::async_trait]
pub trait SandboxHost: Send + Sync {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError>;
    /// The live sandbox carrying this name, if any.
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError>;
    async fn fork(
        &self,
        parent: &str,
        name: &str,
        max_runtime_min: u32,
    ) -> Result<Machine, ProviderError>;
    async fn created_at(&self, id: &str) -> Result<Option<String>, ProviderError>;
    async fn exec(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError>;
    async fn pause(&self, id: &str) -> Result<(), ProviderError>;
    async fn delete(&self, id: &str) -> Result<(), ProviderError>;
}

#[async_trait::async_trait]
impl SandboxHost for Prized {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Provider::inspect(self, id).await
    }
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        Provider::find_allocation(self, name).await
    }
    async fn fork(
        &self,
        parent: &str,
        name: &str,
        max_runtime_min: u32,
    ) -> Result<Machine, ProviderError> {
        Prized::fork_named(self, parent, name, max_runtime_min).await
    }
    async fn created_at(&self, id: &str) -> Result<Option<String>, ProviderError> {
        Prized::created_at(self, id).await
    }
    async fn exec(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        Provider::execute(self, id, command).await
    }
    async fn pause(&self, id: &str) -> Result<(), ProviderError> {
        Provider::sleep(self, id).await.map(|_| ())
    }
    async fn delete(&self, id: &str) -> Result<(), ProviderError> {
        Provider::delete(self, id).await.map(|_| ())
    }
}

pub struct PrizedUserImages {
    host: Arc<dyn SandboxHost>,
}

impl PrizedUserImages {
    pub fn new(host: Arc<dyn SandboxHost>) -> Self {
        Self { host }
    }
    /// Run the guest helper inline: the copy in the journal is used only by
    /// the detached setup runner, never for a check.
    async fn helper(&self, sandbox: &str, args: &[&str]) -> Result<Value, ApiError> {
        let command = format!(
            "/usr/bin/python3 -I -B -c {} {}",
            quote(PRIZED_GUEST_HELPER),
            args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
        );
        let result = self
            .host
            .exec(sandbox, &command)
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
    /// Write a file in exec-sized chunks; the provider's upload call is not
    /// used (it proved unreliable on builders elsewhere).
    async fn write_file(&self, sandbox: &str, path: &str, data: &[u8]) -> Result<(), ApiError> {
        use base64::Engine as _;
        const WRITE: &str = "import base64,os,sys\np,m,d=sys.argv[1:4]\nfd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_NOFOLLOW|(os.O_TRUNC if m=='w' else os.O_APPEND),0o600)\nwith os.fdopen(fd,'wb') as f: f.write(base64.b64decode(d))";
        let chunks: Vec<&[u8]> = if data.is_empty() {
            vec![&[]]
        } else {
            data.chunks(30_000).collect()
        };
        for (n, chunk) in chunks.into_iter().enumerate() {
            let command = format!(
                "/usr/bin/python3 -I -B -c {} {} {} {}",
                quote(WRITE),
                quote(path),
                if n == 0 { "w" } else { "a" },
                quote(&base64::engine::general_purpose::STANDARD.encode(chunk))
            );
            let result = self
                .host
                .exec(sandbox, &command)
                .await
                .map_err(uncertain_here())?;
            if result.exit_code != 0 {
                return Err(ApiError::conflict("user_image_upload_failed"));
            }
        }
        Ok(())
    }
    /// Find the sandbox with this exact name, forking it only when none has
    /// that name and none was recorded. `Ok(Err(()))` means a recorded sandbox
    /// disappeared.
    async fn sandbox(
        &self,
        resources: &mut Value,
        field: &str,
        name: &str,
        parent: &str,
        runtime_min: u32,
    ) -> Result<Result<Machine, ()>, ApiError> {
        let found = match resources[field].as_str() {
            Some(id) => self.host.inspect(id).await,
            None => self.host.find(name).await,
        }
        .map_err(uncertain_here())?;
        let machine = match found {
            Some(machine) => machine,
            None if resources[field].is_null() => self
                .host
                .fork(parent, name, runtime_min)
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
    /// Delete the named sandbox; `true` once it is gone.
    async fn retire(&self, id: Option<&str>, name: &str) -> Result<bool, ApiError> {
        let found = match id {
            Some(id) => self.host.inspect(id).await,
            None => self.host.find(name).await,
        }
        .map_err(uncertain_here())?;
        let Some(machine) = found else {
            return Ok(true);
        };
        if machine.name != name || id.is_some_and(|id| id != machine.id) {
            return Err(ApiError::conflict("user_image_resource_identity_changed"));
        }
        self.host
            .delete(&machine.id)
            .await
            .map_err(uncertain_here())?;
        Ok(self
            .host
            .inspect(&machine.id)
            .await
            .map_err(uncertain_here())?
            .is_none())
    }
    fn tree(resources: &Value) -> Result<String, ApiError> {
        resources["baseTreeSha256"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::conflict("user_image_plan_invalid"))
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
impl UserImageBackend for PrizedUserImages {
    /// A base is a paused sandbox; its creation stamp is its version.
    async fn base_version(
        &self,
        provider: ProviderId,
        artifact: &str,
    ) -> Result<Option<String>, ApiError> {
        if provider != ProviderId::Prized || !artifact.starts_with("sbx_") {
            return Ok(None);
        }
        let parent = self
            .host
            .inspect(artifact)
            .await
            .map_err(uncertain_here())?;
        if parent.is_none_or(|p| p.id != artifact || p.state != MachineState::Paused) {
            return Ok(None);
        }
        self.host
            .created_at(artifact)
            .await
            .map_err(uncertain_here())
    }

    async fn step(&self, build: &Build, mode: StepMode) -> Result<Step, ApiError> {
        let plan = &build.plan;
        if plan.identity.provider != ProviderId::Prized || plan.initial_runtime_bootstrap.is_some()
        {
            return Err(ApiError::conflict("user_image_provider_not_supported"));
        }
        let (builder_name, verifier_name, _) = plan.names();
        let runtime = plan.identity.base_runtime_sha256.as_str();
        let mut resources = build.resources["provider"]
            .as_object()
            .cloned()
            .map(Value::Object)
            .unwrap_or_else(|| json!({}));
        let advance = |phase: &str, resources: Value| {
            Ok(Step::Advance {
                phase: phase.into(),
                resources,
            })
        };
        match build.phase.as_str() {
            "allocate" => {
                let base = &plan.identity.base_artifact_id;
                if self
                    .host
                    .created_at(base)
                    .await
                    .map_err(uncertain_here())?
                    .as_deref()
                    != Some(plan.identity.base_artifact_version.as_str())
                {
                    return Self::failed("user_image_managed_base_pins_mismatch", None, resources);
                }
                let Ok(machine) = self
                    .sandbox(
                        &mut resources,
                        "builderId",
                        &builder_name,
                        base,
                        BUILDER_RUNTIME_MIN,
                    )
                    .await?
                else {
                    return Self::failed("user_image_builder_lost", None, resources);
                };
                if machine.state != MachineState::Running {
                    return Ok(Step::Pending(resources));
                }
                let baseline = self.helper(&machine.id, &["baseline", runtime]).await?;
                let Some(tree) = baseline["treeSha256"]
                    .as_str()
                    .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
                else {
                    // The base itself is not the runtime it was registered with.
                    return Self::failed("user_image_managed_base_pins_mismatch", None, resources);
                };
                resources["baseTreeSha256"] = json!(tree);
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
                    .exec(&id, &format!("install -d -m 700 {JOURNAL}"))
                    .await
                    .map_err(uncertain_here())?;
                if mkdir.exit_code != 0 {
                    return Err(ApiError::conflict("user_image_upload_failed"));
                }
                self.write_file(
                    &id,
                    &format!("{JOURNAL}/setup.sh"),
                    plan.setup_script.as_bytes(),
                )
                .await?;
                self.write_file(
                    &id,
                    &format!("{JOURNAL}/guest.py"),
                    PRIZED_GUEST_HELPER.as_bytes(),
                )
                .await?;
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
                let tree = Self::tree(&resources)?;
                let result = self.helper(&id, &["sanitize", runtime, &tree]).await?;
                if result["sanitized"] == true
                    && result["pinsVerified"] == true
                    && result["runtimeStopped"] == true
                {
                    resources["sanitized"] = json!(true);
                    return advance("capture", resources);
                }
                // A replayed sanitize after a lost reply finds the journal gone;
                // the read-only check decides whether the first run completed.
                let check = self.helper(&id, &["check", runtime, &tree]).await?;
                if check["clean"] == true && mode == StepMode::Reconcile {
                    resources["sanitized"] = json!(true);
                    return advance("capture", resources);
                }
                Self::failed(
                    match result["error"].as_str() {
                        Some(
                            "user_image_pinned_file_changed"
                            | "user_image_pinned_file_unsafe"
                            | "user_image_managed_tree_unsafe"
                            | "user_image_managed_tree_missing",
                        ) => "user_image_runtime_files_changed",
                        Some("user_image_runtime_started") => "user_image_runtime_started",
                        Some("user_image_processes_running") => "user_image_processes_running",
                        _ => "user_image_sanitize_failed",
                    },
                    None,
                    resources,
                )
            }
            "capture" => {
                // The sanitized builder, paused, is the image.
                let id = resources["builderId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_builder_missing"))?
                    .to_owned();
                if resources["sanitized"] != true {
                    resources["captureNotSubmitted"] = json!(true);
                    return Self::failed("user_image_sanitize_failed", None, resources);
                }
                let Some(machine) = self.host.inspect(&id).await.map_err(uncertain_here())? else {
                    return Self::failed("user_image_capture_failed", None, resources);
                };
                if machine.name != builder_name || machine.id != id {
                    return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                }
                if resources["artifactId"].is_null() {
                    let version = self
                        .host
                        .created_at(&id)
                        .await
                        .map_err(uncertain_here())?
                        .ok_or_else(|| ApiError::unavailable("user_image_capture_pending"))?;
                    resources["artifactId"] = json!(id);
                    resources["artifactVersion"] = json!(version);
                }
                match machine.state {
                    MachineState::Paused => advance("retire-builder", resources),
                    MachineState::Running => {
                        self.host.pause(&id).await.map_err(uncertain_here())?;
                        Ok(Step::Pending(resources))
                    }
                    MachineState::Starting => Ok(Step::Pending(resources)),
                    MachineState::Standby | MachineState::Stopped | MachineState::Unknown(_) => {
                        Self::failed("user_image_capture_failed", None, resources)
                    }
                }
            }
            // The builder became the image; nothing to retire.
            "retire-builder" => advance("verify", resources),
            "verify" => {
                let artifact = resources["artifactId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("user_image_artifact_missing"))?
                    .to_owned();
                let Ok(machine) = self
                    .sandbox(
                        &mut resources,
                        "verifierId",
                        &verifier_name,
                        &artifact,
                        VERIFIER_RUNTIME_MIN,
                    )
                    .await?
                else {
                    return Self::failed("user_image_verifier_lost", None, resources);
                };
                if machine.state != MachineState::Running {
                    return Ok(Step::Pending(resources));
                }
                let tree = Self::tree(&resources)?;
                // The copy a computer gets: base runtime and managed files
                // intact, nothing leaked, the idle supervisor answering.
                let check = self.helper(&machine.id, &["check", runtime, &tree]).await?;
                if check["clean"] != true || check["supervisorIdle"] != true {
                    return Self::failed("user_image_verification_failed", None, resources);
                }
                advance("retire-verifier", resources)
            }
            "retire-verifier" => {
                if !self
                    .retire(resources["verifierId"].as_str(), &verifier_name)
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
                let image = self
                    .host
                    .inspect(&artifact)
                    .await
                    .map_err(uncertain_here())?;
                if image.is_none_or(|m| {
                    m.state != MachineState::Paused || m.name != builder_name || m.id != artifact
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
                            self.host.find(name).await.map_err(uncertain_here())?
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
                            // A fork whose reply was lost may still surface:
                            // look once more on the next tick.
                            resources[not_submitted] = json!(true);
                            return Ok(Step::Pending(resources));
                        }
                    }
                }
                if !self
                    .retire(resources["verifierId"].as_str(), &verifier_name)
                    .await?
                    || !self
                        .retire(resources["builderId"].as_str(), &builder_name)
                        .await?
                {
                    return Ok(Step::Pending(resources));
                }
                // The builder was the only copy of the unpublished image.
                resources["builderAbsent"] = json!(true);
                resources["verifierAbsent"] = json!(true);
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
                if let Some(image) = self
                    .host
                    .inspect(&artifact)
                    .await
                    .map_err(uncertain_here())?
                {
                    if image.name != builder_name {
                        return Err(ApiError::conflict("user_image_artifact_identity_changed"));
                    }
                    self.host
                        .delete(&image.id)
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
