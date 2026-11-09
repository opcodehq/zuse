//! Pure encrypted preparation for the original creation transaction. No SQL or
//! provider call is performed here; the four-execution admission batch owns it.
use super::*;
use zuse_cloud_runtime::mailbox::CommandKey;
use zuse_protocol::{
    cloud_commands::{CloudSessionCommand, CommandEnvelope},
    session_commands::SessionCommandPayload,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::runtime) struct InitialPrepared {
    policy_ciphertext: String,
    session_ciphertext: String,
    actor_id: String,
    command: Option<InitialCommand>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitialCommand {
    request_digest: String,
    envelope: CommandEnvelope,
    envelope_bytes: i64,
    presentation_ciphertext: String,
    acceptance: Value,
}
impl RuntimeControl {
    pub(crate) fn with_task_command_creation(mut self, enabled: bool) -> Self {
        self.command_admission_creation = enabled;
        self
    }
    pub(crate) async fn task_command_creation_available(&self) -> Result<bool, ApiError> {
        if !self.command_admission_creation || !self.admission_batch_available().await? {
            return Ok(false);
        }
        self.task_command_delivery_available().await
    }
    /// Schema readiness is independent of allowing new version-two tasks.
    /// Cache the catalog probe: old deployments never parse optional-table SQL.
    pub(in crate::runtime) async fn task_command_delivery_available(
        &self,
    ) -> Result<bool, ApiError> {
        Ok(*self.task_command_schema_ready.get_or_try_init(||async {
            let ready:bool=sqlx::query_scalar("SELECT bool_and(to_regclass(name) IS NOT NULL AND coalesce(has_table_privilege(to_regclass(name),'SELECT'),false) AND coalesce(has_table_privilege(to_regclass(name),'INSERT'),false) AND coalesce(has_table_privilege(to_regclass(name),'UPDATE'),false) AND coalesce(has_table_privilege(to_regclass(name),'DELETE'),false)) AND NOT EXISTS(SELECT 1 FROM (VALUES('zuse_rust_task_command_execution','priority'),('zuse_rust_task_command_execution','wait_for_control_id'),('zuse_rust_task_command_drains','last_error'),('zuse_rust_task_command_queue','edit_lease_hash')) AS required_columns(relation,column_name) WHERE NOT EXISTS(SELECT 1 FROM pg_attribute WHERE attrelid=to_regclass(required_columns.relation) AND attname=required_columns.column_name AND NOT attisdropped)) FROM unnest(ARRAY['zuse_rust_task_command_policies','zuse_rust_task_command_sessions','zuse_rust_task_command_actions','zuse_rust_task_command_execution','zuse_rust_task_command_queue','zuse_rust_task_command_drains','zuse_rust_mailboxes','zuse_rust_mailbox_commands']) AS names(name)").fetch_one(&self.pool).await?;
            Ok::<bool,ApiError>(ready)
        }).await?)
    }
    pub(in crate::runtime) fn prepare_initial_task_command(
        &self,
        row: &Value,
        access: &Access,
        intent: &Value,
        release: &Value,
        now: i64,
    ) -> Result<InitialPrepared, ApiError> {
        if !command_mode(row) {
            return Err(error("server_command_admission_update_required"));
        }
        self.authorize_task_command_row(row, access, true)?;
        let id = required(row, "workspace_id")?;
        let owner = required(row, "account_id")?;
        let session = required(row, "initial_session_id")?;
        let configured = required(&row["request_config"], "agent")?;
        let canonical = if configured == "claude-code" {
            "claude"
        } else {
            configured
        };
        let harness = release["harnesses"]
            .as_array()
            .and_then(|v| v.iter().find(|v| v["agent"] == canonical))
            .ok_or_else(|| error("cloud_task_model_catalog_missing"))?;
        let catalog: zuse_protocol::cloud_catalog::CloudAgentModels =
            serde_json::from_value(harness["catalog"].clone())
                .map_err(|_| error("cloud_task_model_catalog_missing"))?;
        let policy = Policy {
            agent: configured.into(),
            connection_id: required(&row["request_config"], "agentConnectionId")?.into(),
            connection_owner_id: required(&row["request_config"], "agentConnectionOwnerId")?.into(),
            runtime_mode: serde_json::from_value(intent["runtimeMode"].clone())
                .map_err(|_| error("cloud_task_policy_invalid"))?,
            initial_model: required(intent, "model")?.into(),
            reasoning: serde_json::from_value(intent["reasoning"].clone())
                .map_err(|_| error("cloud_task_policy_invalid"))?,
            model_options: intent["modelOptions"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
            catalog,
            catalogs: agents::installed_catalogs(release)?,
        };
        let model = policy
            .catalog
            .resolve_model(&policy.initial_model)
            .ok_or_else(|| error("cloud_task_model_not_installed"))?;
        if policy
            .reasoning
            .is_some_and(|r| !model.reasoning_levels.contains(&r))
        {
            return Err(error("cloud_task_model_reasoning_invalid"));
        }
        for (name, value) in &policy.model_options {
            let choice = value
                .as_str()
                .ok_or_else(|| error("cloud_task_model_option_invalid"))?;
            if !model
                .options
                .iter()
                .any(|option| option.id == *name && option.choices.iter().any(|v| v.id == choice))
            {
                return Err(error("cloud_task_model_option_invalid"));
            }
        }
        let (sandbox, auto) = match policy.runtime_mode {
            zuse_protocol::cloud::CloudRuntimeMode::FullAccess => ("danger-full-access", true),
            zuse_protocol::cloud::CloudRuntimeMode::Auto => ("auto", false),
            _ => ("workspace-write", false),
        };
        let config = json!({"harness":if canonical=="claude"{"claude-code"}else{canonical},"model":policy.initial_model,"reasoning":policy.reasoning,"modelOptions":policy.model_options,"sandbox":sandbox});
        let seal = |kind: &str, key: &str, value: &Value| {
            self.master
                .seal_api(&context(owner, id, session, kind, key), &value.to_string())
                .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))
        };
        let policy_ciphertext = self
            .master
            .seal_api(
                &context(owner, id, "", "policy", "1"),
                &serde_json::to_string(&policy).map_err(|_| error("cloud_task_policy_invalid"))?,
            )
            .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))?;
        let session_ciphertext = seal(
            "session",
            "metadata",
            &json!({"title":intent["title"],"config":config}),
        )?;
        let assets: Vec<zuse_protocol::cloud_assets::Asset> =
            serde_json::from_value(intent.get("attachments").cloned().unwrap_or(json!([])))
                .map_err(|_| error("cloud_task_command_invalid"))?;
        let prompt = intent["firstMessage"].as_str().unwrap_or_default();
        let command = if prompt.trim().is_empty() && assets.is_empty() {
            None
        } else {
            let cid = required(intent, "commandId")?;
            let mid = required(intent, "turnId")?;
            let payload:SessionCommandPayload=serde_json::from_value(json!({"kind":"run","messageId":mid,"request":{"prompt":prompt,"harness":config["harness"],"model":policy.initial_model,"reasoning":policy.reasoning,"modelOptions":policy.model_options,"cwd":".","sandbox":sandbox,"autoApprove":auto,"resume":null,"worktree":null,"mcp":null,"attachments":[]}})).map_err(|_|error("cloud_task_command_invalid"))?;
            let request = wire::Request {
                command_id: cid.into(),
                session_id: session.into(),
                action: wire::Action::Submit {
                    message_id: mid.into(),
                    payload: payload.clone(),
                    attachments: assets.clone(),
                    based_on: None,
                },
                wake: wire::WakePolicy::PreserveSleep,
            };
            validation::request(&request)?;
            let request_text = zeroize::Zeroizing::new(
                serde_json::to_string(&request).map_err(|_| error("cloud_task_command_invalid"))?,
            );
            let request_digest = self
                .master
                .digest_api(&context(owner, id, session, "action", cid), &request_text);
            let native = CloudSessionCommand {
                command_id: cid.into(),
                session_id: session.into(),
                payload,
                based_on: None,
                expires_at: None,
                runtime_binding: Some(wire::RuntimeBinding {
                    version: wire::VERSION,
                    policy_revision: 1,
                    interrupt_command_id: None,
                    interrupt_message_id: None,
                    cwd_relative: ".".into(),
                    attachments: assets.clone(),
                    session: None,
                }),
            };
            let key = self
                .master
                .open_transcript_key(owner, id, required(row, "wrapped_transcript_key")?)
                .map_err(|_| ApiError::unavailable("cloud_command_key_unavailable"))?;
            let key = CommandKey::parse(&key)
                .map_err(|_| ApiError::unavailable("cloud_command_key_unavailable"))?;
            let bytes = zeroize::Zeroizing::new(
                serde_json::to_vec(&native).map_err(|_| error("cloud_task_command_invalid"))?,
            );
            if bytes.len() > 131_072 {
                return Err(ApiError::bad_request("cloud_task_command_too_large"));
            }
            let mut envelope = CommandEnvelope {
                protocol_version: zuse_protocol::cloud_commands::PROTOCOL_VERSION,
                workspace_id: id.into(),
                session_id: session.into(),
                command_id: cid.into(),
                kind: zuse_protocol::cloud_commands::session_command_kind(native.payload.kind())
                    .into(),
                fingerprint: key.fingerprint(&bytes),
                schema_version: 1,
                key_version: 1,
                destruction_fence: super::super::mailbox::destruction_fence(row),
                created_at: now,
                iv: String::new(),
                ciphertext: String::new(),
                dependencies: vec![],
                actor: authorized_actor(access),
            };
            let (iv, ciphertext) = key
                .encrypt(&envelope, &bytes)
                .map_err(|_| ApiError::unavailable("cloud_command_encryption_failed"))?;
            envelope.iv = iv;
            envelope.ciphertext = ciphertext;
            if !envelope.eligible() {
                return Err(ApiError::bad_request("cloud_command_not_eligible"));
            }
            let envelope_bytes = serde_json::to_vec(&envelope)
                .map_err(|_| error("cloud_task_command_invalid"))?
                .len() as i64;
            if envelope_bytes > zuse_protocol::cloud_commands::MAX_MAILBOX_BYTES {
                return Err(error("cloud_mailbox_full"));
            }
            Some(InitialCommand {
                request_digest,
                envelope,
                envelope_bytes,
                presentation_ciphertext: seal(
                    "presentation",
                    cid,
                    &json!({"messageId":mid,"text":prompt,"attachments":assets}),
                )?,
                acceptance: json!({"commandId":cid,"durablyAdmitted":true,"workspaceSequence":1,"revision":1,"acceptedAt":now,"state":"waiting-for-runtime","deliveryState":"waiting-for-runtime","executionState":"queued"}),
            })
        };
        Ok(InitialPrepared {
            policy_ciphertext,
            session_ciphertext,
            actor_id: access.actor.account_id.clone(),
            command,
        })
    }
}
