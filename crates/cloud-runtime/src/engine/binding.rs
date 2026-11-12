//! Resolve only server-sealed operational bindings after mailbox authentication.
//! Cloud paths never originate on the user's Mac or pass through an opaque RPC.
use super::*;
mod agent;
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
impl EngineExecutor {
    pub(super) async fn resolve_task_binding(
        &self,
        payload: &CloudSessionCommand,
    ) -> Result<CloudSessionCommand> {
        let Some(binding) = &payload.runtime_binding else {
            ensure!(
                self.task_context.is_none(),
                "cloud_canonical_command_binding_required"
            );
            return Ok(payload.clone());
        };
        ensure!(
            binding.interrupt_command_id.is_some() == binding.interrupt_message_id.is_some(),
            "cloud_interrupt_target_invalid"
        );
        if !matches!(payload.payload, SessionCommandPayload::Interrupt {}) {
            ensure!(
                binding.interrupt_command_id.is_none(),
                "cloud_interrupt_target_invalid"
            );
        }
        let context = self
            .task_context
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("cloud_command_admission_update_required"))?;
        let boot = context.updates.borrow().clone();
        ensure!(
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
            "cloud_command_policy_or_runtime_fenced"
        );
        ensure!(
            relative(&binding.cwd_relative)
                && std::fs::canonicalize(&self.checkout)? == context.checkout,
            "cloud_command_checkout_fenced"
        );
        let cwd = std::fs::canonicalize(context.checkout.join(&binding.cwd_relative))?;
        ensure!(
            cwd.starts_with(&context.checkout) && cwd.is_dir(),
            "cloud_command_checkout_fenced"
        );
        let mut resolved = payload.clone();
        if let SessionCommandPayload::Run { request, .. } = &mut resolved.payload {
            ensure!(
                request.cwd == binding.cwd_relative
                    && request.resume.is_none()
                    && request.worktree.is_none()
                    && request.mcp.is_none(),
                "cloud_canonical_command_binding_required"
            );
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
                .ok_or_else(|| anyhow::anyhow!("cloud_command_thread_missing"))?;
            let config = root
                .config
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("cloud_command_permission_policy_missing"))?;
            ensure!(
                request.sandbox == config.sandbox
                    && request.auto_approve == (config.sandbox == SandboxLevel::DangerFullAccess),
                "cloud_command_permission_policy_changed"
            );
            if let Some(registration) = &binding.session {
                ensure!(
                    registration.session_id == payload.session_id
                        && registration.thread_id == self.thread,
                    "cloud_conversation_scope_rejected"
                );
                self.core.workspace.install_registered_thread_session(
                    &root,
                    registration,
                    &zuse_protocol::ChatConfig {
                        harness: selected,
                        model: request.model.clone(),
                        reasoning: request.reasoning,
                        model_options: request.model_options.clone(),
                        sandbox: request.sandbox,
                        plan: request.plan,
                    },
                )?;
            }
            self.conversation(&payload.session_id)?;
            // Files uploaded straight to this computer for this message: each
            // must be one of its own uploads.
            let uploads = std::fs::canonicalize(self.core.uploads.dir())?;
            let mut paths =
                Vec::with_capacity(request.attachments.len() + binding.attachments.len());
            for path in &request.attachments {
                let file = std::fs::canonicalize(path)
                    .map_err(|_| anyhow::anyhow!("cloud_command_upload_missing"))?;
                ensure!(
                    file.starts_with(&uploads) && file.is_file(),
                    "cloud_command_upload_outside_uploads"
                );
                paths.push(file.to_string_lossy().into_owned());
            }
            ensure!(
                binding.attachments.len() <= zuse_protocol::cloud_assets::MAX_ASSETS
                    && binding
                        .attachments
                        .iter()
                        .try_fold(0usize, |sum, a| sum.checked_add(a.size_bytes))
                        .is_some_and(|total| total <= zuse_protocol::cloud_assets::MAX_BYTES),
                "cloud_command_assets_too_large"
            );
            for asset in &binding.attachments {
                let (verified, bytes) = context.authority.asset(&boot, asset).await?;
                paths.push(self.core.uploads.import_cloud_asset(&verified, &bytes)?);
            }
            request.prompt =
                zuse_protocol::message_attachments::with_attachments(&request.prompt, &paths);
            request.attachments = paths;
            request.cwd = cwd.to_string_lossy().into_owned();
        } else {
            ensure!(
                binding.attachments.is_empty() && binding.session.is_none(),
                "cloud_control_runtime_binding_invalid"
            );
        }
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
