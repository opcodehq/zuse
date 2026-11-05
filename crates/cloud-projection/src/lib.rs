//! Engine-free encrypted read projections. These values never admit commands or replace engine storage.
#[cfg(not(target_arch = "wasm32"))]
mod cache;
#[cfg(not(target_arch = "wasm32"))]
mod secure_cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod protected_metadata;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
#[cfg(test)]
use zuse_model::{SessionMessageEntry, TranscriptUpdate};
use zuse_model::{TranscriptBaseline, TranscriptFrame};
#[cfg(test)]
use zuse_protocol::Chat;

pub const FORMAT: &str = "zuse-rust-transcript-v1";
pub const HEAD_LIMIT: usize = 1024 * 1024;
pub const PAGE_LIMIT: usize = 4 * 1024 * 1024;
pub const PLAIN_LIMIT: usize = 32 * 1024 * 1024;
pub use zuse_model::cloud::{Cursor, Head, Page};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub session_id: String,
    pub cursor: Cursor,
    pub ciphertext: String,
    pub ciphertext_sha256: String,
    pub before_sequence: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_format: Option<String>,
}
/// Does not implement Debug or Serialize, and owns a zeroized copy of the workspace key.
pub struct TranscriptKey(zeroize::Zeroizing<Vec<u8>>);
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    compression: String,
    iv: String,
    ciphertext: String,
}
pub fn digest(bytes: &[u8]) -> String {
    B64.encode(Sha256::digest(bytes))
}
fn aad(workspace: &str, session: &str, cursor: &Cursor, page: Option<i64>) -> Vec<u8> {
    format!(
        "zuse-cloud-transcript-v1\n{workspace}\n{session}\n{}\n{}\n1\n{}",
        cursor.epoch,
        cursor.version,
        page.map(|n| format!("page:{n}"))
            .unwrap_or_else(|| "head".into())
    )
    .into_bytes()
}
impl TranscriptKey {
    pub fn parse(encoded: &str) -> Result<Self> {
        let bytes = zeroize::Zeroizing::new(B64.decode(encoded)?);
        ensure!(bytes.len() == 32, "invalid_transcript_key");
        Ok(Self(bytes))
    }
    pub fn seal(
        &self,
        workspace: &str,
        session: &str,
        cursor: &Cursor,
        page: Option<i64>,
        value: &impl Serialize,
    ) -> Result<Upload> {
        let bytes = serde_json::to_vec(value)?;
        ensure!(
            bytes.len() <= PLAIN_LIMIT,
            "transcript_projection_too_large"
        );
        let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
        gzip.write_all(&bytes)?;
        let compressed = gzip.finish()?;
        let mut iv = [0; 12];
        OsRng.fill_bytes(&mut iv);
        let ciphertext = Aes256Gcm::new_from_slice(&self.0)
            .map_err(|_| anyhow::anyhow!("invalid_transcript_key"))?
            .encrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &compressed,
                    aad: &aad(workspace, session, cursor, page),
                },
            )
            .map_err(|_| anyhow::anyhow!("transcript_encryption_failed"))?;
        let ciphertext = serde_json::to_string(&Envelope {
            version: 1,
            compression: "gzip".into(),
            iv: B64.encode(iv),
            ciphertext: B64.encode(ciphertext),
        })?;
        ensure!(
            ciphertext.len()
                <= if page.is_some() {
                    PAGE_LIMIT
                } else {
                    HEAD_LIMIT
                },
            "transcript_projection_too_large"
        );
        Ok(Upload {
            session_id: session.into(),
            cursor: cursor.clone(),
            ciphertext_sha256: digest(ciphertext.as_bytes()),
            ciphertext,
            before_sequence: page,
            payload_format: Some(FORMAT.into()),
        })
    }
    pub fn open<T: serde::de::DeserializeOwned>(
        &self,
        workspace: &str,
        session: &str,
        cursor: &Cursor,
        page: Option<i64>,
        ciphertext: &str,
    ) -> Result<T> {
        ensure!(
            ciphertext.len()
                <= if page.is_some() {
                    PAGE_LIMIT
                } else {
                    HEAD_LIMIT
                },
            "transcript_projection_too_large"
        );
        let env: Envelope = serde_json::from_str(ciphertext)?;
        ensure!(
            env.version == 1 && env.compression == "gzip",
            "unsupported_transcript_ciphertext"
        );
        let iv = B64.decode(env.iv)?;
        ensure!(iv.len() == 12, "invalid_transcript_nonce");
        let bytes = B64.decode(env.ciphertext)?;
        let plain = Aes256Gcm::new_from_slice(&self.0)
            .map_err(|_| anyhow::anyhow!("invalid_transcript_key"))?
            .decrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &bytes,
                    aad: &aad(workspace, session, cursor, page),
                },
            )
            .map_err(|_| anyhow::anyhow!("transcript_authentication_failed"))?;
        let mut decoded = Vec::new();
        GzDecoder::new(plain.as_slice())
            .take((PLAIN_LIMIT + 1) as u64)
            .read_to_end(&mut decoded)?;
        ensure!(
            decoded.len() <= PLAIN_LIMIT,
            "transcript_projection_too_large"
        );
        Ok(serde_json::from_slice(&decoded)?)
    }
}
/// Retain complete native message/part types rather than translating or dropping tool state.
pub fn encode(mut head: Head, key: &TranscriptKey) -> Result<Vec<Upload>> {
    let TranscriptFrame::Reset { reset: entries } = &head.transcript.frame else {
        anyhow::bail!("projection_requires_full_snapshot")
    };
    let all = entries.clone();
    let mut count = all.len().min(64);
    let upload = loop {
        let start = all.len() - count;
        head.transcript.frame = TranscriptFrame::Reset {
            reset: all[start..].to_vec().into(),
        };
        head.transcript.replay_baseline = Some(TranscriptBaseline::capture(&all[start..]));
        head.older_message_sequence = (start > 0).then_some(start as i64);
        match key.seal(
            &head.workspace_id,
            &head.session_id,
            &head.cursor,
            None,
            &head,
        ) {
            Ok(upload) => break upload,
            Err(_) if count > 0 => count /= 2,
            Err(e) => return Err(e),
        }
    };
    let mut uploads = vec![upload];
    let mut end = all.len() - count;
    while end > 0 {
        let mut count = end.min(64);
        loop {
            let start = end - count;
            let page = Page {
                schema_version: 1,
                payload_format: FORMAT.into(),
                workspace_id: head.workspace_id.clone(),
                session_id: head.session_id.clone(),
                runtime_generation: head.runtime_generation,
                cursor: head.cursor.clone(),
                before_sequence: end as i64,
                entries: all[start..end].to_vec(),
                older_message_sequence: (start > 0).then_some(start as i64),
            };
            match key.seal(
                &head.workspace_id,
                &head.session_id,
                &head.cursor,
                Some(end as i64),
                &page,
            ) {
                Ok(upload) => {
                    uploads.push(upload);
                    end = start;
                    break;
                }
                Err(_) if count > 1 => count /= 2,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(uploads)
}
pub mod reader;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn opens_original_node_ciphertext_and_rejects_changed_scope_and_digest() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../cloud/tests/fixtures/transcript-crypto.json"
        ))
        .unwrap();
        let key = TranscriptKey::parse(fixture["transcriptKey"].as_str().unwrap()).unwrap();
        let upload: Upload = serde_json::from_value(fixture["head2"].clone()).unwrap();
        let value: serde_json::Value = key
            .open(
                "ws-bob",
                "session-bob",
                &upload.cursor,
                None,
                &upload.ciphertext,
            )
            .unwrap();
        assert_eq!(
            value["projection"]["messages"][0]["content"]["text"],
            "Latest preserved user text"
        );
        assert!(
            key.open::<serde_json::Value>(
                "other-workspace",
                "session-bob",
                &upload.cursor,
                None,
                &upload.ciphertext
            )
            .is_err()
        );
        assert!(
            key.open::<serde_json::Value>(
                "ws-bob",
                "session-bob",
                &upload.cursor,
                Some(10),
                &upload.ciphertext
            )
            .is_err()
        );
    }
    #[test]
    fn paging_retains_all_native_entries_and_history_boundaries() {
        let key = TranscriptKey::parse(&B64.encode([9; 32])).unwrap();
        let chat:Chat=serde_json::from_value(json!({"id":"session","deviceId":"device","title":"Cached","archived":true,"cwd":"/repo","branch":"feature","checkoutId":null,"config":null,"lastMessagePreview":null,"lastMessageAt":null,"createdAt":"2026-10-23T00:00:00Z"})).unwrap();
        let entries: Vec<SessionMessageEntry> = (0..150)
            .map(|i| SessionMessageEntry {
                id: format!("message-{i}"),
                role: zuse_model::MessageRole::Assistant,
                parts: vec![
                    zuse_model::MessagePart::Text {
                        id: format!("part-{i}"),
                        text: format!("Retained output {i}"),
                    },
                    zuse_model::MessagePart::Reasoning {
                        id: format!("reasoning-{i}"),
                        text: format!("Retained reasoning {i}"),
                    },
                ],
                created_at: i,
                device_id: "device".into(),
                status: None,
                continuation_of: None,
                duration_ms: Some(i),
            })
            .collect();
        let head = Head {
            schema_version: 1,
            payload_format: FORMAT.into(),
            workspace_id: "workspace".into(),
            session_id: "session".into(),
            runtime_generation: 2,
            cursor: Cursor {
                epoch: "retained-epoch".into(),
                version: 3,
            },
            chat: chat.clone(),
            conversations: vec![chat],
            transcript: TranscriptUpdate {
                frame: TranscriptFrame::Reset {
                    reset: entries.clone().into(),
                },
                context_usage: None,
                replay_baseline: None,
            },
            queue: json!([{"id":"held-message","text":"Resume later"}]),
            queue_paused: true,
            older_message_sequence: None,
        };
        let uploads = encode(head.clone(), &key).unwrap();
        assert_eq!(uploads.len(), 3);
        let decoded: Head = key
            .open(
                "workspace",
                "session",
                &head.cursor,
                None,
                &uploads[0].ciphertext,
            )
            .unwrap();
        assert_eq!(decoded.older_message_sequence, Some(86));
        assert!(decoded.queue_paused);
        assert_eq!(decoded.queue, head.queue);
        let TranscriptFrame::Reset { reset: actual } = decoded.transcript.frame else {
            panic!()
        };
        let mut actual = std::sync::Arc::unwrap_or_clone(actual);
        for upload in &uploads[1..] {
            let page: Page = key
                .open(
                    "workspace",
                    "session",
                    &head.cursor,
                    upload.before_sequence,
                    &upload.ciphertext,
                )
                .unwrap();
            let mut older = page.entries;
            older.append(&mut actual);
            actual = older;
        }
        assert_eq!(actual, entries);
        assert!(!uploads[0].ciphertext.contains("Retained output"));
    }
}
