//! Resolve only server-sealed operational bindings after mailbox authentication.
//! Cloud paths never originate on the user's Mac or pass through an opaque RPC.
use super::*;
mod agent;
use crate::mailbox::{reject, require};
use std::path::{Component, Path};
use tokio::sync::watch;
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap, cloud_commands::CloudSessionCommand, cloud_task_commands as wire,
    session_commands::SessionCommandPayload,
};
#[derive(Clone)]
pub struct TaskContext {
    authority: crate::Authority,
    updates: watch::Receiver<CloudRuntimeBootstrap>,
    workspace: String,
    generation: i64,
    disk: String,
    checkout: PathBuf,
}
impl TaskContext {
    pub fn new(
        authority: crate::Authority,
        updates: watch::Receiver<CloudRuntimeBootstrap>,
        checkout: &Path,
    ) -> Result<Self> {
        let boot = updates.borrow().clone();
        ensure!(
            canonical_mode(&boot),
            "cloud_command_admission_update_required"
        );
        let disk = boot
            .runtime_storage
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("cloud_runtime_storage_missing"))?
            .storage_incarnation_id
            .clone();
        Ok(Self {
            authority,
            updates,
            workspace: boot.workspace_id,
            generation: boot.runtime_generation,
            disk,
            checkout: std::fs::canonicalize(checkout)?,
        })
    }
}
fn canonical_mode(boot: &CloudRuntimeBootstrap) -> bool {
    boot.extra.get("commandDeliveryMode") == Some(&serde_json::json!(wire::MODE))
        && boot.extra.get("commandAdmissionVersion") == Some(&serde_json::json!(wire::VERSION))
}
fn relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.contains('\\')
        && !value.contains('\0')
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|v| matches!(v, Component::Normal(_) | Component::CurDir))
}
/// A task command that passed every check made before this computer changes
/// anything for it.
pub(super) struct Checked {
    boot: CloudRuntimeBootstrap,
    cwd: PathBuf,
    run: Option<CheckedRun>,
}
struct CheckedRun {
    selected: zuse_protocol::HarnessId,
    root: zuse_protocol::Chat,
    config: zuse_protocol::ChatConfig,
    uploads: Vec<String>,
}
impl EngineExecutor {
    /// Checks a task command's binding while only reading: nothing in the
    /// engine, the conversation, its settings or the uploads is written
    /// before this returns. Its refusals are therefore definitive
    /// ([`crate::mailbox::AdmissionRejected`]); only a failed store read
    /// stays uncertain.
    pub(super) fn check_task_binding(
        &self,
        payload: &CloudSessionCommand,
    ) -> Result<Option<Checked>> {
        let Some(binding) = &payload.runtime_binding else {
            require(
                self.task_context.is_none(),
                "cloud_canonical_command_binding_required",
            )?;
            return Ok(None);
        };
        require(
            binding.interrupt_command_id.is_some() == binding.interrupt_message_id.is_some(),
            "cloud_interrupt_target_invalid",
        )?;
        if !matches!(payload.payload, SessionCommandPayload::Interrupt {}) {
            require(
                binding.interrupt_command_id.is_none(),
                "cloud_interrupt_target_invalid",
            )?;
        }
        let context = self
            .task_context
            .as_ref()
            .ok_or_else(|| reject("cloud_command_admission_update_required"))?;
        let boot = context.updates.borrow().clone();
        require(
            canonical_mode(&boot)
                && boot.workspace_id == context.workspace
                && boot.runtime_generation == context.generation
                && boot.chat_id == self.thread
                && boot
                    .runtime_storage
                    .as_ref()
                    .is_some_and(|s| s.storage_incarnation_id == context.disk)
                && binding.version == wire::VERSION
                && binding.policy_revision > 0
                && boot.extra.get("commandPolicyRevision")
                    == Some(&serde_json::json!(binding.policy_revision)),
            "cloud_command_policy_or_runtime_fenced",
        )?;
        require(
            relative(&binding.cwd_relative)
                && std::fs::canonicalize(&self.checkout).ok().as_ref() == Some(&context.checkout),
            "cloud_command_checkout_fenced",
        )?;
        let cwd = std::fs::canonicalize(context.checkout.join(&binding.cwd_relative))
            .map_err(|_| reject("cloud_command_checkout_fenced"))?;
        require(
            cwd.starts_with(&context.checkout) && cwd.is_dir(),
            "cloud_command_checkout_fenced",
        )?;
        let SessionCommandPayload::Run { request, .. } = &payload.payload else {
            require(
                binding.attachments.is_empty() && binding.session.is_none(),
                "cloud_control_runtime_binding_invalid",
            )?;
            return Ok(Some(Checked {
                boot,
                cwd,
                run: None,
            }));
        };
        require(
            request.cwd == binding.cwd_relative
                && request.resume.is_none()
                && request.worktree.is_none()
                && request.mcp.is_none(),
            "cloud_canonical_command_binding_required",
        )?;
        let selected = agent::resolve(&boot, request, |harness| {
            self.core
                .registry
                .resolve(harness)
                .is_ok_and(|driver| driver.installed())
        })?;
        let root = self
            .core
            .workspace
            .chat(&self.thread)?
            .ok_or_else(|| reject("cloud_command_thread_missing"))?;
        let config = root
            .config
            .clone()
            .ok_or_else(|| reject("cloud_command_permission_policy_missing"))?;
        require(
            request.auto_approve == (request.sandbox == SandboxLevel::DangerFullAccess),
            "cloud_command_permission_policy_changed",
        )?;
        if let Some(registration) = &binding.session {
            require(
                registration.session_id == payload.session_id
                    && registration.thread_id == self.thread,
                "cloud_conversation_scope_rejected",
            )?;
        }
        // Files uploaded straight to this computer for this message: each
        // must be one of its own uploads. Together with files bound by the
        // server they stay within the per-message limit.
        require(
            request.attachments.len() + binding.attachments.len()
                <= zuse_protocol::cloud_assets::MAX_ASSETS,
            "cloud_command_assets_too_many",
        )?;
        let mut uploads = Vec::with_capacity(request.attachments.len());
        if !request.attachments.is_empty() {
            let dir = std::fs::canonicalize(self.core.uploads.dir())
                .map_err(|_| reject("cloud_command_upload_missing"))?;
            for path in &request.attachments {
                let file = std::fs::canonicalize(path)
                    .map_err(|_| reject("cloud_command_upload_missing"))?;
                require(
                    file.starts_with(&dir) && file.is_file(),
                    "cloud_command_upload_outside_uploads",
                )?;
                uploads.push(file.to_string_lossy().into_owned());
            }
        }
        require(
            binding
                .attachments
                .iter()
                .try_fold(0usize, |sum, a| sum.checked_add(a.size_bytes))
                .is_some_and(|total| total <= zuse_protocol::cloud_assets::MAX_BYTES),
            "cloud_command_assets_too_large",
        )?;
        Ok(Some(Checked {
            boot,
            cwd,
            run: Some(CheckedRun {
                selected,
                root,
                config,
                uploads,
            }),
        }))
    }
    /// Applies a checked binding. From here on a failure may follow a write,
    /// so it stays uncertain.
    pub(super) async fn bind_task(
        &self,
        payload: &CloudSessionCommand,
        checked: Option<Checked>,
    ) -> Result<CloudSessionCommand> {
        let mut resolved = payload.clone();
        let (
            Some(Checked {
                boot,
                cwd,
                run: Some(run),
            }),
            Some(binding),
            Some(context),
        ) = (checked, &payload.runtime_binding, &self.task_context)
        else {
            return Ok(resolved);
        };
        let SessionCommandPayload::Run { request, .. } = &mut resolved.payload else {
            return Ok(resolved);
        };
        // Access is chosen per message and the server has already admitted
        // this one under the task policy: carry the mode into the thread.
        if request.sandbox != run.config.sandbox {
            let mut updated = run.config.clone();
            updated.sandbox = request.sandbox;
            self.core
                .workspace
                .set_chat_config(&self.thread, &updated)?;
        }
        if let Some(registration) = &binding.session {
            self.core.workspace.install_registered_thread_session(
                &run.root,
                registration,
                &zuse_protocol::ChatConfig {
                    harness: run.selected,
                    model: request.model.clone(),
                    reasoning: request.reasoning,
                    model_options: request.model_options.clone(),
                    sandbox: request.sandbox,
                    plan: request.plan,
                },
            )?;
        }
        self.conversation(&payload.session_id)?;
        let mut paths = run.uploads;
        for asset in &binding.attachments {
            let (verified, bytes) = context.authority.asset(&boot, asset).await?;
            paths.push(self.core.uploads.import_cloud_asset(&verified, &bytes)?);
        }
        request.prompt =
            zuse_protocol::message_attachments::with_attachments(&request.prompt, &paths);
        request.attachments = paths;
        request.cwd = cwd.to_string_lossy().into_owned();
        Ok(resolved)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_cwd_never_accepts_a_sender_absolute_or_parent_path() {
        for path in [
            "/Users/owner/repo",
            "../other",
            "a/../../other",
            r"C:\repo",
            "a\0b",
        ] {
            assert!(!relative(path));
        }
        for path in [".", "src", "packages/app"] {
            assert!(relative(path));
        }
    }
}
