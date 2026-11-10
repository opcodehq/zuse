//! A read-only resume hint. A mismatch always falls back to authoritative history.
use crate::{SessionMessageEntry, TranscriptFrame, TranscriptUpsert};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptResume {
    first: String,
    count: usize,
    sha256: String,
}
struct HashWriter(Sha256);
impl std::io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn digest(entries: &[SessionMessageEntry]) -> String {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, entries).expect("transcript JSON is serializable");
    format!("{:x}", writer.0.finalize())
}
impl TranscriptResume {
    pub fn capture(entries: &[SessionMessageEntry]) -> Option<Self> {
        Some(Self {
            first: entries.first()?.id.clone(),
            count: entries.len(),
            sha256: digest(entries),
        })
    }
    pub fn frame(&self, current: &[SessionMessageEntry]) -> Option<TranscriptFrame> {
        if self.count == 0 || self.count > current.len() || self.sha256.len() != 64 {
            return None;
        }
        let start = current.iter().position(|entry| entry.id == self.first)?;
        let end = start.checked_add(self.count)?;
        if end > current.len() || digest(&current[start..end]) != self.sha256 {
            return None;
        }
        // The validated middle is already on the client. Inserting before its
        // first row implicitly moves that row; do not resend its body.
        let upsert = current
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < start || *i >= end)
            .map(|(i, entry)| TranscriptUpsert {
                after: i.checked_sub(1).map(|p| current[p].id.clone()),
                entry: entry.clone(),
            })
            .collect();
        Some(TranscriptFrame::Delta {
            upsert,
            append: Vec::new(),
            remove: Vec::new(),
            count: current.len(),
        })
    }
}
