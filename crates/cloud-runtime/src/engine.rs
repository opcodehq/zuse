//! Native engine admission shared by the executable and cloud acceptance.
use anyhow::{Result, ensure};
use std::{path::PathBuf, sync::Arc};
use zuse_engine::EngineCore;
use zuse_protocol::SandboxLevel;

pub struct EngineExecutor {
    pub core: Arc<EngineCore>,
    pub thread: String,
    pub checkout: PathBuf,
    pub allow_test: bool,
}
impl EngineExecutor {
    fn conversation(&self, session: &str) -> Result<zuse_protocol::Chat> {
        let chat = self
            .core
            .workspace
            .chat(session)?
            .ok_or_else(|| anyhow::anyhow!("mailbox_conversation_missing"))?;
        ensure!(
            chat.id == self.thread || chat.thread_id.as_deref() == Some(&self.thread),
            "mailbox_conversation_scope_rejected"
        );
        ensure!(
            !chat.archived && chat.device_id == self.core.device_id,
            "mailbox_conversation_unavailable"
        );
        Ok(chat)
    }
}
#[async_trait::async_trait]
impl crate::mailbox::Executor for EngineExecutor {
    async fn native_admitted(
        &self,
        payload: &zuse_protocol::cloud_commands::CloudSessionCommand,
        fingerprint: &str,
    ) -> Result<bool> {
        let Some(chat) = self.core.workspace.chat(&payload.session_id)? else {
            return Ok(false);
        };
        ensure!(
            chat.id == self.thread || chat.thread_id.as_deref() == Some(&self.thread),
            "mailbox_conversation_scope_rejected"
        );
        Ok(self.core.doc_host.mailbox_command_admitted(
            &payload.session_id,
            &payload.command_id,
            fingerprint,
        )?)
    }
    async fn admit_native(
        &self,
        payload: &zuse_protocol::cloud_commands::CloudSessionCommand,
        fingerprint: &str,
        created_at: i64,
    ) -> Result<()> {
        self.conversation(&payload.session_id)?;
        if let zuse_protocol::session_commands::SessionCommandPayload::Run {
            request,
            message_id,
        } = &payload.payload
        {
            ensure!(
                !message_id.is_empty() && message_id.len() <= 512,
                "mailbox_message_identity_invalid"
            );
            ensure!(!request.cwd.is_empty(), "mailbox_checkout_missing");
            let harness = request.harness.or_else(|| {
                self.core
                    .workspace
                    .chat_config(&payload.session_id)
                    .map(|config| config.harness)
            });
            ensure!(
                harness != Some(zuse_protocol::HarnessId::Mock) || self.allow_test,
                "runtime_test_harness_disabled"
            );
        }
        let entry = zuse_doc::SessionCommandEntry {
            id: payload.command_id.clone(),
            payload: payload.payload.clone(),
            issued_by: format!("cloud-mailbox:{fingerprint}"),
            issued_at: created_at,
            based_on: payload.based_on.clone(),
            expires_at: Some(payload.expires_at.unwrap_or(9_007_199_254_740_991)),
            status: zuse_doc::SessionCommandStatus::Pending,
            resolution: None,
        };
        self.core
            .doc_host
            .admit_mailbox_command(&payload.session_id, entry)
            .await?;
        Ok(())
    }
    async fn admitted(
        &self,
        payload: &crate::mailbox::MessageSend,
        fingerprint: &str,
    ) -> Result<bool> {
        let Some(chat) = self.core.workspace.chat(&payload.session_id)? else {
            return Ok(false);
        };
        ensure!(
            chat.id == self.thread || chat.thread_id.as_deref() == Some(&self.thread),
            "mailbox_conversation_scope_rejected"
        );
        Ok(self.core.doc_host.mailbox_command_admitted(
            &payload.session_id,
            &payload.command_id,
            fingerprint,
        )?)
    }
    async fn admit(
        &self,
        payload: &crate::mailbox::MessageSend,
        fingerprint: &str,
        created_at: i64,
    ) -> Result<()> {
        let chat = self
            .core
            .workspace
            .chat(&payload.session_id)?
            .ok_or_else(|| anyhow::anyhow!("mailbox_conversation_missing"))?;
        ensure!(
            chat.id == self.thread || chat.thread_id.as_deref() == Some(&self.thread),
            "mailbox_conversation_scope_rejected"
        );
        ensure!(
            !chat.archived && chat.device_id == self.core.device_id,
            "mailbox_conversation_unavailable"
        );
        let config = self
            .core
            .workspace
            .chat_config(&chat.id)
            .or_else(|| self.core.workspace.chat_config(&self.thread))
            .ok_or_else(|| anyhow::anyhow!("mailbox_conversation_settings_missing"))?;
        ensure!(
            config.harness != zuse_protocol::HarnessId::Mock || self.allow_test,
            "runtime_test_harness_disabled"
        );
        let cwd = chat
            .cwd
            .as_deref()
            .unwrap_or_else(|| self.checkout.to_str().unwrap_or(""));
        ensure!(!cwd.is_empty(), "mailbox_checkout_missing");
        let model_options = match &payload.model_options {
            Some(options) => serde_json::to_value(options)?,
            None => serde_json::to_value(&config.model_options)?,
        };
        let entry: zuse_doc::SessionCommandEntry = serde_json::from_value(
            serde_json::json!({"id":payload.command_id,"payload":{"kind":"run","messageId":payload.client_message_id,"request":{"prompt":payload.prompt(),"harness":config.harness,"model":config.model,"reasoning":config.reasoning,"modelOptions":model_options,"cwd":cwd,"sandbox":config.sandbox,"plan":config.plan,"autoApprove":config.sandbox==SandboxLevel::DangerFullAccess}},"issuedBy":format!("cloud-mailbox:{fingerprint}"),"issuedAt":created_at,"expiresAt":9_007_199_254_740_991i64,"status":"pending"}),
        )?;
        self.core
            .doc_host
            .admit_mailbox_command(&payload.session_id, entry)
            .await?;
        Ok(())
    }
}
