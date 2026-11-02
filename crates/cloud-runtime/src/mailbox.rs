//! Encrypted command delivery into the existing engine queue and SQLite receipts.
use crate::{Authority, authority::Failure, now_ms};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, Payload, rand_core::RngCore},
};
use anyhow::{Result, bail, ensure};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use hmac::{Hmac, Mac};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap,
    cloud_commands::{
        AcknowledgmentState, CloudSessionCommand, CommandEnvelope, LeasePage,
        RuntimeAcknowledgment, RuntimeLease,
    },
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSend {
    pub command_id: String,
    pub session_id: String,
    pub client_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<ComposerInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_goal: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_options: Option<BTreeMap<String, String>>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposerInput {
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Value>,
    #[serde(default)]
    pub file_refs: Vec<Value>,
    #[serde(default)]
    pub skill_refs: Vec<Value>,
    #[serde(default)]
    pub annotations: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_goal: Option<bool>,
}
impl MessageSend {
    pub fn prompt(&self) -> &str {
        self.input
            .as_ref()
            .map(|v| v.text.as_str())
            .or(self.text.as_deref())
            .unwrap_or("")
    }
    fn limitation(&self) -> Option<&'static str> {
        if self.as_goal == Some(true)
            || self.input.as_ref().is_some_and(|v| v.as_goal == Some(true))
        {
            return Some("command-goal-mode-not-supported");
        }
        if let Some(input) = &self.input {
            if !input.attachments.is_empty() {
                return Some("command-dependencies-not-supported");
            }
            if !input.file_refs.is_empty()
                || !input.skill_refs.is_empty()
                || !input.annotations.is_empty()
            {
                return Some("command-context-import-required");
            }
        }
        None
    }
}
enum DecodedCommand {
    Message(Box<MessageSend>),
    Session(Box<CloudSessionCommand>),
}
impl DecodedCommand {
    fn expired(&self) -> bool {
        matches!(self,Self::Session(command) if command.expires_at.is_some_and(|expiry|expiry<=now_ms()))
    }
}
#[async_trait]
pub trait Executor: Send + Sync {
    /// Read proof only. A missing proof must never execute work without a fresh lease.
    async fn admitted(&self, payload: &MessageSend, fingerprint: &str) -> Result<bool>;
    async fn native_admitted(
        &self,
        payload: &CloudSessionCommand,
        fingerprint: &str,
    ) -> Result<bool>;
    async fn admit_native(
        &self,
        payload: &CloudSessionCommand,
        fingerprint: &str,
        created_at: i64,
    ) -> Result<()>;
    /// Must durably admit to the canonical engine queue, retaining the command ID.
    async fn admit(&self, payload: &MessageSend, fingerprint: &str, created_at: i64) -> Result<()>;
}
pub struct CommandKey(Zeroizing<[u8; 32]>);
impl CommandKey {
    pub fn parse(encoded: &str) -> Result<Self> {
        let bytes = Zeroizing::new(
            B64.decode(encoded)
                .map_err(|_| anyhow::anyhow!("command-key-invalid"))?,
        );
        Ok(Self(Zeroizing::new(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("command-key-invalid"))?,
        )))
    }
    pub fn fingerprint(&self, plaintext: &[u8]) -> String {
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(self.0.as_slice()).expect("32-byte key");
        mac.update(plaintext);
        format!("hmac-sha256:{}", B64.encode(mac.finalize().into_bytes()))
    }
    pub fn encrypt(
        &self,
        envelope: &CommandEnvelope,
        plaintext: &[u8],
    ) -> Result<(String, String)> {
        ensure!(plaintext.len() <= 128 * 1024, "command-payload-too-large");
        let cipher = Aes256Gcm::new_from_slice(self.0.as_slice()).expect("32-byte key");
        let mut iv = [0u8; 12];
        OsRng.fill_bytes(&mut iv);
        let bytes = cipher
            .encrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: plaintext,
                    aad: &envelope.additional_data(),
                },
            )
            .map_err(|_| anyhow::anyhow!("command-encryption-failed"))?;
        Ok((B64.encode(iv), B64.encode(bytes)))
    }
    pub fn decrypt(&self, envelope: &CommandEnvelope) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(envelope.eligible(), "command-schema-not-supported");
        let iv = B64
            .decode(&envelope.iv)
            .map_err(|_| anyhow::anyhow!("command-decryption-failed"))?;
        ensure!(iv.len() == 12, "command-decryption-failed");
        let ciphertext = B64
            .decode(&envelope.ciphertext)
            .map_err(|_| anyhow::anyhow!("command-decryption-failed"))?;
        let cipher = Aes256Gcm::new_from_slice(self.0.as_slice()).expect("32-byte key");
        let bytes = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&iv),
                    Payload {
                        msg: &ciphertext,
                        aad: &envelope.additional_data(),
                    },
                )
                .map_err(|_| anyhow::anyhow!("command-decryption-failed"))?,
        );
        ensure!(bytes.len() <= 128 * 1024, "command-payload-too-large");
        // Verify the keyed identity independently of AES-GCM's authenticated metadata.
        let tag = envelope
            .fingerprint
            .strip_prefix("hmac-sha256:")
            .and_then(|v| B64.decode(v).ok())
            .ok_or_else(|| anyhow::anyhow!("command-fingerprint-mismatch"))?;
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(self.0.as_slice()).expect("32-byte key");
        mac.update(&bytes);
        mac.verify_slice(&tag)
            .map_err(|_| anyhow::anyhow!("command-fingerprint-mismatch"))?;
        Ok(bytes)
    }
}
struct Receipts(Mutex<Connection>);
impl Receipts {
    fn open(path: &Path) -> Result<Self> {
        // Storage admission has already verified this existing database. Never create another one.
        let connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS zuse_cloud_command_receipts (command_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL,session_id TEXT NOT NULL,storage_incarnation_id TEXT NOT NULL,fingerprint TEXT NOT NULL,envelope TEXT NOT NULL,lease_json TEXT NOT NULL,acknowledgment TEXT,delivered INTEGER NOT NULL DEFAULT 0);")?;
        Ok(Self(Mutex::new(connection)))
    }
    fn bind(&self, lease: &RuntimeLease) -> Result<Option<RuntimeAcknowledgment>> {
        let db = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime-receipt-store-unavailable"))?;
        db.execute("INSERT OR IGNORE INTO zuse_cloud_command_receipts(command_id,workspace_id,session_id,storage_incarnation_id,fingerprint,envelope,lease_json) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![lease.command.command_id,lease.command.workspace_id,lease.command.session_id,lease.storage_incarnation_id,lease.command.fingerprint,serde_json::to_string(&lease.command)?,serde_json::to_string(lease)?])?;
        let row:(String,String,String,String,Option<String>)=db.query_row("SELECT workspace_id,session_id,storage_incarnation_id,fingerprint,acknowledgment FROM zuse_cloud_command_receipts WHERE command_id=?1",[&lease.command.command_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
        ensure!(
            row.0 == lease.command.workspace_id
                && row.1 == lease.command.session_id
                && row.2 == lease.storage_incarnation_id
                && row.3 == lease.command.fingerprint,
            "runtime-receipt-invalid"
        );
        db.execute(
            "UPDATE zuse_cloud_command_receipts SET lease_json=?2 WHERE command_id=?1",
            params![lease.command.command_id, serde_json::to_string(lease)?],
        )?;
        row.4
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    fn save(&self, ack: &RuntimeAcknowledgment) -> Result<()> {
        let db = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime-receipt-store-unavailable"))?;
        let changed=db.execute("UPDATE zuse_cloud_command_receipts SET acknowledgment=?2,delivered=0 WHERE command_id=?1 AND fingerprint=?3",params![ack.command_id,serde_json::to_string(ack)?,ack.fingerprint])?;
        ensure!(changed == 1, "runtime-receipt-invalid");
        Ok(())
    }
    fn unfinished(&self) -> Result<Vec<RuntimeLease>> {
        let db = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime-receipt-store-unavailable"))?;
        let mut statement=db.prepare("SELECT lease_json FROM zuse_cloud_command_receipts WHERE acknowledgment IS NULL LIMIT 256")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|v| Ok(serde_json::from_str(&v?)?))
            .collect()
    }
    fn pending(&self) -> Result<Vec<RuntimeAcknowledgment>> {
        let db = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("runtime-receipt-store-unavailable"))?;
        let mut statement=db.prepare("SELECT acknowledgment FROM zuse_cloud_command_receipts WHERE acknowledgment IS NOT NULL AND delivered=0 LIMIT 256")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|v| Ok(serde_json::from_str(&v?)?))
            .collect()
    }
    fn delivered(&self, ack: &RuntimeAcknowledgment) -> Result<()> {
        self.0.lock().map_err(|_|anyhow::anyhow!("runtime-receipt-store-unavailable"))?.execute("UPDATE zuse_cloud_command_receipts SET delivered=1 WHERE command_id=?1 AND acknowledgment=?2",params![ack.command_id,serde_json::to_string(ack)?])?;
        Ok(())
    }
}
pub struct Mailbox {
    key: CommandKey,
    receipts: Receipts,
    executor: Arc<dyn Executor>,
    workspace: String,
    incarnation: String,
    sandbox: String,
    generation: i64,
}
impl Mailbox {
    pub fn new(
        path: &Path,
        bootstrap: &CloudRuntimeBootstrap,
        key: CommandKey,
        executor: Arc<dyn Executor>,
    ) -> Result<Self> {
        Ok(Self {
            key,
            receipts: Receipts::open(path)?,
            executor,
            workspace: bootstrap.workspace_id.clone(),
            incarnation: bootstrap
                .runtime_storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("runtime-storage-missing"))?
                .storage_incarnation_id
                .clone(),
            sandbox: bootstrap
                .extra
                .get("providerSandboxId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("runtime-provider-sandbox-missing"))?
                .into(),
            generation: bootstrap.runtime_generation,
        })
    }
    async fn admitted(&self, command: &DecodedCommand, fingerprint: &str) -> Result<bool> {
        match command {
            DecodedCommand::Message(command) => self.executor.admitted(command, fingerprint).await,
            DecodedCommand::Session(command) => {
                self.executor.native_admitted(command, fingerprint).await
            }
        }
    }
    async fn admit(
        &self,
        command: &DecodedCommand,
        fingerprint: &str,
        created_at: i64,
    ) -> Result<()> {
        match command {
            DecodedCommand::Message(command) => {
                self.executor.admit(command, fingerprint, created_at).await
            }
            DecodedCommand::Session(command) => {
                self.executor
                    .admit_native(command, fingerprint, created_at)
                    .await
            }
        }
    }
    async fn recover(&self) -> Result<()> {
        for lease in self.receipts.unfinished()? {
            ensure!(
                lease.command.workspace_id == self.workspace
                    && lease.storage_incarnation_id == self.incarnation,
                "runtime-receipt-invalid"
            );
            if let Ok(payload) = self.decode(&lease.command)
                && self.admitted(&payload, &lease.command.fingerprint).await?
            {
                let result = json!({"commandId":lease.command.command_id,"result":null});
                let (iv, ciphertext) = self
                    .key
                    .encrypt(&lease.command, result.to_string().as_bytes())?;
                self.receipts.save(&RuntimeAcknowledgment {
                    command_id: lease.command.command_id,
                    lease_token: lease.lease_token,
                    fingerprint: lease.command.fingerprint,
                    state: AcknowledgmentState::Applied,
                    category: None,
                    result_iv: Some(iv),
                    result_ciphertext: Some(ciphertext),
                })?;
            }
        }
        Ok(())
    }
    pub async fn apply(&self, lease: &RuntimeLease) -> Result<Option<RuntimeAcknowledgment>> {
        ensure!(
            lease.command.workspace_id == self.workspace
                && lease.runtime_generation == self.generation
                && lease.provider_sandbox_id == self.sandbox
                && lease.storage_incarnation_id == self.incarnation,
            "runtime-mailbox-lease-fenced"
        );
        let existing = self.receipts.bind(lease)?;
        if let Some(mut ack) = existing {
            ack.lease_token = lease.lease_token.clone();
            self.receipts.save(&ack)?;
            return Ok(Some(ack));
        }
        if now_ms() >= lease.lease_deadline {
            return Ok(None);
        }
        let mut ack = RuntimeAcknowledgment {
            command_id: lease.command.command_id.clone(),
            lease_token: lease.lease_token.clone(),
            fingerprint: lease.command.fingerprint.clone(),
            state: AcknowledgmentState::Rejected,
            category: None,
            result_iv: None,
            result_ciphertext: None,
        };
        let payload = self.decode(&lease.command);
        match payload {
            Ok(payload) => {
                if payload.expired() {
                    ack.state = AcknowledgmentState::Expired;
                    ack.category = Some("command-expired".into());
                    self.receipts.save(&ack)?;
                    return Ok(Some(ack));
                }
                let result = json!({"commandId":lease.command.command_id,"result":null});
                let (iv, ciphertext) = self
                    .key
                    .encrypt(&lease.command, result.to_string().as_bytes())?;
                if now_ms() >= lease.lease_deadline {
                    return Ok(None);
                }
                match self
                    .admit(
                        &payload,
                        &lease.command.fingerprint,
                        lease.command.created_at,
                    )
                    .await
                {
                    Ok(()) => {
                        ack.state = AcknowledgmentState::Applied;
                        ack.result_iv = Some(iv);
                        ack.result_ciphertext = Some(ciphertext);
                    }
                    Err(_) => {
                        ack.state = AcknowledgmentState::OutcomeUnknown;
                        ack.category = Some("runtime-admission-not-proven".into());
                    }
                }
            }
            Err(category) => ack.category = Some(category.into()),
        }
        self.receipts.save(&ack)?;
        Ok(Some(ack))
    }
    fn decode(
        &self,
        envelope: &CommandEnvelope,
    ) -> std::result::Result<DecodedCommand, &'static str> {
        if envelope.actor.is_some() {
            return Err("command-author-access-unavailable");
        }
        let bytes = self
            .key
            .decrypt(envelope)
            .map_err(|_| "command-decryption-failed")?;
        if zuse_protocol::cloud_commands::is_session_command_kind(&envelope.kind) {
            let command: CloudSessionCommand =
                serde_json::from_slice(&bytes).map_err(|_| "command-payload-invalid")?;
            if command.command_id != envelope.command_id
                || command.session_id != envelope.session_id
            {
                return Err("command-payload-identity-mismatch");
            }
            if zuse_protocol::cloud_commands::session_command_kind(command.payload.kind())
                != envelope.kind
            {
                return Err("command-kind-mismatch");
            }
            if command
                .expires_at
                .is_some_and(|expiry| !(0..=9_007_199_254_740_991).contains(&expiry))
            {
                return Err("command-expiry-invalid");
            }
            return Ok(DecodedCommand::Session(Box::new(command)));
        }
        let payload: MessageSend =
            serde_json::from_slice(&bytes).map_err(|_| "command-payload-invalid")?;
        if payload.command_id != envelope.command_id
            || payload.session_id != envelope.session_id
            || payload.client_message_id.is_empty()
            || payload.client_message_id.len() > 512
            || (payload.text.is_none() && payload.input.is_none())
        {
            return Err("command-payload-identity-mismatch");
        }
        if let Some(category) = payload.limitation() {
            return Err(category);
        }
        Ok(DecodedCommand::Message(Box::new(payload)))
    }
    pub async fn run(
        &self,
        authority: Authority,
        bootstrap: watch::Receiver<CloudRuntimeBootstrap>,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.recover().await?;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let boot = bootstrap.borrow().clone();
            // A lost HTTP response replays only the committed receipt, never the engine call.
            for ack in self.receipts.pending()? {
                match authority
                    .post(
                        &self.workspace,
                        &["runtime", "commands", "ack"],
                        &boot.runtime_credential,
                        &serde_json::to_value(&ack)?,
                    )
                    .await
                {
                    Ok(_) => self.receipts.delivered(&ack)?,
                    Err(error) if code(&error) == Some("stale-runtime-lease") => {
                        self.receipts.delivered(&ack)?
                    }
                    Err(error) if retry(&error) => {
                        crate::supervisor::wait(&cancel, 1).await;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            let request = json!({"storageIncarnationId":self.incarnation,"waitMs":5000});
            let page = tokio::select! {_=cancel.cancelled()=>return Ok(()),page=authority.post(&self.workspace,&["runtime","commands","lease"],&boot.runtime_credential,&request)=>page};
            let page: LeasePage = match page {
                Ok(page) => serde_json::from_value(page)
                    .map_err(|_| anyhow::anyhow!("runtime-mailbox-page-invalid"))?,
                Err(error) if retry(&error) => {
                    crate::supervisor::wait(&cancel, 1).await;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if page.fence_required {
                bail!("runtime_mailbox_fence_required");
            }
            for lease in page.leases {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                let _ = self.apply(&lease).await?;
            }
        }
    }
}
fn code(error: &anyhow::Error) -> Option<&'static str> {
    error.downcast_ref::<Failure>().map(|v| v.code)
}
fn retry(error: &anyhow::Error) -> bool {
    crate::supervisor::retryable(error)
        || matches!(
            code(error),
            Some(
                "cloud_workspace_runtime_not_ready"
                    | "cloud_command_mailbox_disabled"
                    | "workspace_runtime_fenced"
                    | "runtime_authority_fenced"
            )
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn webcrypto(input: Value) -> Value {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let mut child = Command::new("node")
            .args([
                "--input-type=module",
                "-e",
                include_str!("../tests/fixtures/mailbox-webcrypto.mjs"),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "WebCrypto compatibility helper failed"
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    #[test]
    fn original_webcrypto_mailbox_ciphertext_and_keyed_identity_interoperate() {
        let encoded = B64.encode([31; 32]);
        let key = CommandKey::parse(&encoded).unwrap();
        let envelope = json!({"protocolVersion":3,"workspaceId":"workspace-original","sessionId":"session-original","commandId":"command-original","kind":"messages.send","fingerprint":"","schemaVersion":1,"keyVersion":1,"destructionFence":7,"createdAt":1790000000000i64,"iv":"","ciphertext":"","dependencies":[]});
        let plaintext=json!({"commandId":"command-original","sessionId":"session-original","clientMessageId":"message-original","text":"Original command ☁"}).to_string();
        let payload: MessageSend = serde_json::from_str(&plaintext).unwrap();
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            serde_json::from_str::<Value>(&plaintext).unwrap(),
            "Original optional fields must be omitted instead of replaced with null"
        );
        let node =
            webcrypto(json!({"encodedKey":encoded,"envelope":envelope,"plaintext":plaintext}));
        let mut envelope: CommandEnvelope =
            serde_json::from_value(node["envelope"].clone()).unwrap();
        assert_eq!(
            key.decrypt(&envelope).unwrap().as_slice(),
            plaintext.as_bytes()
        );
        assert_eq!(key.fingerprint(plaintext.as_bytes()), envelope.fingerprint);
        let (iv, ciphertext) = key.encrypt(&envelope, plaintext.as_bytes()).unwrap();
        envelope.iv = iv;
        envelope.ciphertext = ciphertext;
        let decoded = webcrypto(
            json!({"mode":"decrypt","encodedKey":encoded,"envelope":envelope,"plaintext":null}),
        );
        assert_eq!(decoded["plaintext"], plaintext);
        let mut tampered = envelope.clone();
        tampered.session_id = "other-session".into();
        assert!(key.decrypt(&tampered).is_err());
        let mut tampered = envelope;
        tampered.fingerprint = format!("hmac-sha256:{}", B64.encode([32; 32]));
        let (iv, ciphertext) = key.encrypt(&tampered, plaintext.as_bytes()).unwrap();
        tampered.iv = iv;
        tampered.ciphertext = ciphertext;
        assert!(
            key.decrypt(&tampered).is_err(),
            "Valid AES-GCM alone cannot replace the keyed command identity"
        );
    }
}
