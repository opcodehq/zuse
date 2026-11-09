//! Durable read-only tool publication. It never replays an agent/tool operation.
use serde::{Deserialize, Serialize};

pub const CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolOutputChunk {
    pub blob_ref: String,
    pub offset: usize,
    pub next_offset: usize,
    pub total_bytes: usize,
    pub text: String,
}
impl std::fmt::Debug for ToolOutputChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolOutputChunk")
            .field("offset", &self.offset)
            .field("bytes", &self.text.len())
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}
pub fn valid_ref(reference: &str) -> bool {
    reference.split_once('/').is_some_and(|(chat, part)| {
        !chat.is_empty()
            && chat.len() <= 128
            && chat
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            && !part.is_empty()
            && part.len() <= 200
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:#~-".contains(&b))
    })
}
impl ToolOutputChunk {
    pub fn valid(&self) -> bool {
        valid_ref(&self.blob_ref)
            && self.text.len() <= CHUNK_BYTES
            && self.total_bytes <= MAX_BYTES
            && self.offset.checked_add(self.text.len()) == Some(self.next_offset)
            && self.next_offset <= self.total_bytes
            && (self.next_offset == self.total_bytes || !self.text.is_empty())
    }
}

#[cfg(feature = "engine")]
pub mod guest {
    use super::*;
    use anyhow::{Result, ensure};
    use rusqlite::{Connection, OptionalExtension, params};
    use std::path::Path;
    use zuse_protocol::cloud::CloudRuntimeBootstrap;
    fn open(database: &Path) -> Result<Connection> {
        let db =
            Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS zuse_cloud_tool_output_outbox(blob_ref TEXT NOT NULL,offset INTEGER NOT NULL,payload TEXT NOT NULL,delivered INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(blob_ref,offset));CREATE TABLE IF NOT EXISTS zuse_cloud_tool_output_captured(blob_ref TEXT PRIMARY KEY,total_bytes INTEGER NOT NULL);")?;
        Ok(db)
    }
    pub fn capture(
        core: &zuse_engine::EngineCore,
        database: &Path,
        session: &str,
        entries: &impl Serialize,
    ) -> Result<()> {
        let entries = serde_json::to_value(entries)?;
        let mut refs = std::collections::BTreeSet::new();
        for entry in entries.as_array().into_iter().flatten() {
            for part in entry["parts"].as_array().into_iter().flatten() {
                if part["kind"] != "tool" || part["resolved"] != true {
                    continue;
                }
                for field in ["outputRef", "diffRef"] {
                    if let Some(reference) = part[field].as_str().filter(|r| {
                        valid_ref(r) && r.split_once('/').is_some_and(|(chat, _)| chat == session)
                    }) {
                        refs.insert(reference.to_owned());
                    }
                }
            }
        }
        let mut db = open(database)?;
        for reference in refs {
            let captured: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM zuse_cloud_tool_output_captured WHERE blob_ref=?1)",
                [&reference],
                |r| r.get(0),
            )?;
            if captured {
                continue;
            }
            let Some(first) =
                core.doc_host
                    .read_local_tool_blob_page(&reference, 0, CHUNK_BYTES)?
            else {
                continue;
            };
            ensure!(first.total_bytes <= MAX_BYTES, "tool_output_too_large");
            let tx = db.transaction()?;
            let mut page = first;
            let mut offset = 0;
            loop {
                let chunk = ToolOutputChunk {
                    blob_ref: reference.clone(),
                    offset,
                    next_offset: page.next_offset,
                    total_bytes: page.total_bytes,
                    text: page.text,
                };
                ensure!(chunk.valid(), "tool_output_page_invalid");
                tx.execute("INSERT OR IGNORE INTO zuse_cloud_tool_output_outbox(blob_ref,offset,payload) VALUES(?1,?2,?3)",params![reference,offset,serde_json::to_string(&chunk)?])?;
                if page.done {
                    tx.execute("INSERT INTO zuse_cloud_tool_output_captured(blob_ref,total_bytes) VALUES(?1,?2)",params![reference,chunk.total_bytes])?;
                    break;
                }
                offset = chunk.next_offset;
                page = core
                    .doc_host
                    .read_local_tool_blob_page(&reference, offset, CHUNK_BYTES)?
                    .ok_or_else(|| anyhow::anyhow!("tool_output_missing"))?;
                ensure!(
                    page.total_bytes == chunk.total_bytes,
                    "tool_output_changed_during_publication"
                );
            }
            tx.commit()?;
        }
        Ok(())
    }
    pub fn pending(database: &Path) -> Result<bool> {
        Ok(open(database)?.query_row(
            "SELECT EXISTS(SELECT 1 FROM zuse_cloud_tool_output_outbox WHERE delivered=0)",
            [],
            |row| row.get(0),
        )?)
    }
    pub async fn publish(
        database: &Path,
        authority: &crate::Authority,
        boot: &CloudRuntimeBootstrap,
        budget: usize,
    ) -> Result<()> {
        for _ in 0..budget {
            let next:Option<(String,usize,String)>=open(database)?.query_row("SELECT blob_ref,offset,payload FROM zuse_cloud_tool_output_outbox WHERE delivered=0 ORDER BY blob_ref,offset LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let Some((reference, offset, payload)) = next else {
                return Ok(());
            };
            authority
                .post(
                    &boot.workspace_id,
                    &["runtime", "tool-output"],
                    &boot.runtime_credential,
                    &serde_json::from_str(&payload)?,
                )
                .await?;
            open(database)?.execute("UPDATE zuse_cloud_tool_output_outbox SET delivered=1 WHERE blob_ref=?1 AND offset=?2 AND payload=?3",params![reference,offset,payload])?;
        }
        Ok(())
    }
}
