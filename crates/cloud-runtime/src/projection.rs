//! Guest-owned encrypted projection outbox. Commands and writable engine snapshots never leave through this path.
use crate::Authority;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use zuse_cloud_projection::{Cursor, FORMAT, Head, TranscriptKey, Upload, digest};
use zuse_engine::{EngineCore, doc_host::ChatDocHandle};
use zuse_protocol::cloud::CloudRuntimeBootstrap;

pub struct Publisher {
    database: PathBuf,
    key: TranscriptKey,
    observed: Mutex<HashMap<String, Vec<u8>>>,
}
impl Publisher {
    pub fn open(database: &Path, encoded_key: &str) -> Result<Self> {
        let publisher = Self {
            database: database.into(),
            key: TranscriptKey::parse(encoded_key)?,
            observed: Mutex::new(HashMap::new()),
        };
        publisher.connect()?.execute_batch("CREATE TABLE IF NOT EXISTS zuse_cloud_projection_outbox (session_id TEXT PRIMARY KEY,epoch TEXT NOT NULL,version INTEGER NOT NULL,fingerprint TEXT NOT NULL,uploads TEXT,next_index INTEGER NOT NULL DEFAULT 0)")?;
        Ok(publisher)
    }
    fn connect(&self) -> Result<Connection> {
        ensure!(self.database.is_file(), "runtime_storage_missing");
        let db = Connection::open_with_flags(
            &self.database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "synchronous", "FULL")?;
        Ok(db)
    }
    pub fn capture(
        &self,
        core: &EngineCore,
        boot: &CloudRuntimeBootstrap,
        handles: &[Arc<ChatDocHandle>],
    ) -> Result<()> {
        let mut chats = crate::lifecycle::family(core, boot)?
            .into_iter()
            .map(|id| {
                core.workspace
                    .chat(&id)?
                    .ok_or_else(|| anyhow::anyhow!("projection_conversation_missing"))
            })
            .collect::<Result<Vec<_>>>()?;
        chats.sort_by(|a, b| a.id.cmp(&b.id));
        let family = serde_json::to_vec(&chats)?;
        for handle in handles {
            let doc = handle.doc_arc();
            let session = doc
                .chat_id()
                .ok_or_else(|| anyhow::anyhow!("projection_conversation_missing"))?;
            let id = session.as_str();
            let Some(chat) = chats.iter().find(|c| c.id == id).cloned() else {
                anyhow::bail!("projection_conversation_scope_mismatch")
            };
            let before = doc.doc().oplog_vv();
            let mut signature = before.encode();
            signature.extend_from_slice(&family);
            if self.observed.lock().unwrap().get(id) == Some(&signature) {
                continue;
            }
            let mut db = self.connect()?;
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let prior:Option<(String,i64,String,Option<String>)>=tx.query_row("SELECT epoch,version,fingerprint,uploads FROM zuse_cloud_projection_outbox WHERE session_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            if prior.as_ref().is_some_and(|p| p.3.is_some()) {
                continue;
            }
            let epoch = prior
                .as_ref()
                .map(|p| p.0.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let mut head = Head {
                slack_results: doc.slack_turn_results(),
                schema_version: 1,
                payload_format: FORMAT.into(),
                workspace_id: boot.workspace_id.clone(),
                session_id: id.into(),
                runtime_generation: boot.runtime_generation,
                cursor: Cursor { epoch, version: 0 },
                chat,
                conversations: chats.clone(),
                transcript: zuse_doc::TranscriptUpdate {
                    frame: zuse_doc::TranscriptFrame::Reset {
                        reset: doc.read_entries()?.into(),
                    },
                    context_usage: doc.context_usage(),
                    replay_baseline: None,
                },
                queue: serde_json::to_value(doc.read_queue()?)?,
                queue_paused: doc.queue_paused().unwrap_or(false),
                older_message_sequence: None,
            };
            if before != doc.doc().oplog_vv() {
                continue;
            }
            let fingerprint = digest(&serde_json::to_vec(&head)?);
            if prior.as_ref().is_some_and(|p| p.2 == fingerprint) {
                self.observed.lock().unwrap().insert(id.into(), signature);
                continue;
            }
            head.cursor.version = prior.as_ref().map_or(1, |p| p.1.saturating_add(1));
            ensure!(
                head.cursor.version <= 9_007_199_254_740_991,
                "projection_cursor_exhausted"
            );
            let uploads = zuse_cloud_projection::encode(head.clone(), &self.key)?;
            tx.execute("INSERT INTO zuse_cloud_projection_outbox(session_id,epoch,version,fingerprint,uploads,next_index) VALUES(?1,?2,?3,?4,?5,0) ON CONFLICT(session_id) DO UPDATE SET version=excluded.version,fingerprint=excluded.fingerprint,uploads=excluded.uploads,next_index=0",params![id,head.cursor.epoch,head.cursor.version,fingerprint,serde_json::to_string(&uploads)?])?;
            tx.commit()?;
            self.observed.lock().unwrap().insert(id.into(), signature);
        }
        Ok(())
    }
    pub async fn publish(&self, authority: &Authority, boot: &CloudRuntimeBootstrap) -> Result<()> {
        loop {
            let pending:Option<(String,String,i64)>=self.connect()?.query_row("SELECT session_id,uploads,next_index FROM zuse_cloud_projection_outbox WHERE uploads IS NOT NULL ORDER BY session_id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let Some((session, raw, index)) = pending else {
                return Ok(());
            };
            let uploads: Vec<Upload> = serde_json::from_str(&raw)?;
            ensure!(
                index >= 0 && (index as usize) < uploads.len(),
                "projection_journal_invalid"
            );
            let upload = &uploads[index as usize];
            let suffix = if upload.before_sequence.is_some() {
                "transcript-message-page"
            } else {
                "transcript-checkpoint"
            };
            authority
                .post(
                    &boot.workspace_id,
                    &["runtime", suffix],
                    &boot.runtime_credential,
                    &serde_json::to_value(upload)?,
                )
                .await?;
            let saved=self.connect()?.execute("UPDATE zuse_cloud_projection_outbox SET next_index=?2,uploads=CASE WHEN ?2=?3 THEN NULL ELSE uploads END WHERE session_id=?1 AND uploads=?4 AND next_index=?5",params![session,index+1,uploads.len() as i64,raw,index])?;
            ensure!(saved == 1, "projection_journal_contended");
        }
    }
    pub async fn finish(
        &self,
        core: &EngineCore,
        authority: &Authority,
        boot: &CloudRuntimeBootstrap,
        handles: &[Arc<ChatDocHandle>],
    ) -> Result<()> {
        self.publish(authority, boot).await?;
        self.capture(core, boot, handles)?;
        self.publish(authority, boot).await
    }
    pub async fn run(
        &self,
        core: Arc<EngineCore>,
        authority: Authority,
        credentials: tokio::sync::watch::Receiver<CloudRuntimeBootstrap>,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=cancel.cancelled()=>return,_=interval.tick()=>{}};
            let boot = credentials.borrow().clone();
            let result = async {
                self.publish(&authority, &boot).await?;
                let handles = crate::lifecycle::family(&core, &boot)?
                    .iter()
                    .map(|id| core.doc_host.open_local(id).map_err(Into::into))
                    .collect::<Result<Vec<_>>>()?;
                self.capture(&core, &boot, &handles)?;
                self.publish(&authority, &boot).await
            };
            tokio::select! {_=cancel.cancelled()=>return,result=result=>if result.is_err(){tracing::warn!("Encrypted transcript publication deferred; durable outbox retained");}}
        }
    }
}
