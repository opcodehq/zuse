//! Original encrypted transcript contracts. Cached reads have no lifecycle/provider dependency.
use crate::{access::Access, crypto::MasterKey, error::ApiError, objects::Objects, store::Store};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const HEAD_LIMIT: usize = 1024 * 1024;
pub const HOT_HEAD_LIMIT: usize = 256 * 1024;
pub const PAGE_LIMIT: usize = 4 * 1024 * 1024;
pub use zuse_cloud_projection::{Cursor, Upload};
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadQuery {
    pub epoch: Option<String>,
    pub version: Option<String>,
    pub before_sequence: Option<String>,
    pub format: Option<String>,
}
pub struct Reader {
    pub(crate) objects: Arc<dyn Objects>,
    pub(crate) master: MasterKey,
}
pub fn digest(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}
fn safe(n: i64) -> bool {
    (0..=9_007_199_254_740_991).contains(&n)
}
fn number(s: Option<&str>) -> Option<i64> {
    let raw = s.unwrap_or("0").trim();
    let value = if raw.is_empty() {
        0.0
    } else {
        raw.parse::<f64>().ok()?
    };
    (value.is_finite() && value.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&value))
        .then_some(value as i64)
}
pub(crate) fn component(s: &str) -> String {
    s.as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(b) {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub fn object_key(
    workspace: &str,
    session: &str,
    generation: i64,
    cursor: &Cursor,
    page: Option<i64>,
) -> String {
    let base = format!(
        "workspaces/{}/sessions/{}/generations/{}/{}/{}",
        component(workspace),
        component(session),
        generation,
        component(&cursor.epoch),
        cursor.version
    );
    match page {
        Some(n) => format!("{base}/pages/{n}.json"),
        None => format!("{base}.json"),
    }
}
pub fn formatted_key(
    workspace: &str,
    session: &str,
    generation: i64,
    cursor: &Cursor,
    page: Option<i64>,
    native: bool,
) -> String {
    let key = object_key(workspace, session, generation, cursor, page);
    if native {
        key.replacen(
            &format!("/generations/{generation}/"),
            &format!("/generations/{generation}/rust-v1/"),
            1,
        )
    } else {
        key
    }
}
pub fn validate_upload(upload: &Upload, page: bool) -> Result<(), ApiError> {
    if upload.session_id.is_empty()
        || upload.session_id.len() > 512
        || upload.cursor.epoch.is_empty()
        || upload.cursor.epoch.len() > 512
        || upload
            .session_id
            .chars()
            .chain(upload.cursor.epoch.chars())
            .any(char::is_control)
        || !safe(upload.cursor.version)
        || upload.ciphertext.is_empty()
        || upload.ciphertext.len() > if page { PAGE_LIMIT } else { HEAD_LIMIT }
        || digest(upload.ciphertext.as_bytes()) != upload.ciphertext_sha256
        || page != upload.before_sequence.is_some()
        || upload.before_sequence.is_some_and(|n| n < 1 || !safe(n))
    {
        return Err(ApiError::bad_request(if page {
            "invalid_transcript_message_page"
        } else {
            "invalid_transcript_checkpoint"
        }));
    }
    if upload
        .payload_format
        .as_deref()
        .is_some_and(|f| f != zuse_cloud_projection::FORMAT)
    {
        return Err(ApiError::bad_request("unsupported_transcript_format"));
    }
    Ok(())
}

impl Store {
    pub fn with_transcripts(mut self, objects: Arc<dyn Objects>, master: MasterKey) -> Self {
        self.transcripts = Some(Arc::new(Reader { objects, master }));
        self
    }
    pub fn configure_transcripts(self) -> anyhow::Result<Self> {
        let Some(objects) = crate::objects::R2::from_env()? else {
            return Ok(self);
        };
        let encoded = std::env::var("CLOUD_DATA_ENCRYPTION_KEY")
            .or_else(|_| std::env::var("CLOUD_CREDENTIAL_VAULT_KEY"))
            .map_err(|_| anyhow::anyhow!("Transcript reads require the existing cloud data key"))?;
        let master =
            MasterKey::parse(&encoded).map_err(|_| anyhow::anyhow!("Invalid cloud data key"))?;
        Ok(self.with_transcripts(objects, master))
    }
    pub fn transcripts_enabled(&self) -> bool {
        self.transcripts.is_some()
    }
    pub async fn transcript_read(
        &self,
        access: &Access,
        id: &str,
        session: &str,
        query: ReadQuery,
        page: bool,
    ) -> Result<Value, ApiError> {
        let workspace = self.workspace(access, id).await?;
        if matches!(
            workspace.row["state"].as_str(),
            Some("deleted" | "deleting")
        ) || workspace.row["desired_state"] == "deleted"
        {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        let reader = self
            .transcripts
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_transcript_store_unavailable"))?;
        let empty = if page {
            json!({"page":null})
        } else {
            json!({"checkpoint":null})
        };
        let metadata:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(t) FROM api_cloud_transcript_checkpoints t WHERE workspace_id=$1 AND session_id=$2").bind(id).bind(session).fetch_optional(&self.pool).await?;
        let Some(metadata) = metadata else {
            return Ok(empty);
        };
        let epoch = metadata["stream_epoch"].as_str().ok_or_else(invalid)?;
        let version = metadata["stream_version"].as_i64().ok_or_else(invalid)?;
        let generation = metadata["runtime_generation"]
            .as_i64()
            .ok_or_else(invalid)?;
        let cursor = Cursor {
            epoch: epoch.into(),
            version,
        };
        let before = number(query.before_sequence.as_deref()).filter(|n| *n > 0);
        if page {
            if query.epoch.as_deref() != Some(epoch)
                || number(query.version.as_deref()) != Some(version)
                || before.is_none()
            {
                return Ok(empty);
            }
        } else if query.epoch.as_deref() == Some(epoch)
            && number(query.version.as_deref()).is_some_and(|v| v >= version)
        {
            return Ok(empty);
        }
        let native =
            metadata["object_key"] == formatted_key(id, session, generation, &cursor, None, true);
        if native && query.format.as_deref() != Some(zuse_cloud_projection::FORMAT) {
            return Err(ApiError::conflict("cloud_transcript_protocol_mismatch"));
        }
        let expected = formatted_key(id, session, generation, &cursor, None, native);
        if metadata["object_key"] != expected {
            return Err(invalid());
        }
        let key = if page {
            formatted_key(id, session, generation, &cursor, before, native)
        } else {
            expected
        };
        let hot = &metadata["rust_hot_head"];
        let inline = (!page
            && hot["runtimeGeneration"] == metadata["runtime_generation"]
            && hot["epoch"] == metadata["stream_epoch"]
            && hot["version"] == metadata["stream_version"]
            && hot["sha256"] == metadata["ciphertext_sha256"])
            .then(|| hot["ciphertext"].as_str())
            .flatten()
            .filter(|value| value.len() <= HOT_HEAD_LIMIT);
        let bytes = match inline {
            Some(value) => Some(value.as_bytes().to_vec()),
            None => reader.objects.get(&key).await?,
        };
        let Some(bytes) = bytes else {
            return if page {
                Ok(empty)
            } else {
                Err(ApiError::unavailable("cloud_transcript_checkpoint_missing"))
            };
        };
        if (!page
            && (metadata["ciphertext_sha256"] != digest(&bytes)
                || metadata["ciphertext_bytes"].as_u64() != Some(bytes.len() as u64)))
            || bytes.len() > if page { PAGE_LIMIT } else { HEAD_LIMIT }
        {
            return Err(ApiError::unavailable("cloud_transcript_integrity_failed"));
        }
        let ciphertext = String::from_utf8(bytes).map_err(|_| invalid())?;
        // Recheck sharing/deletion after object I/O before disclosing the transcript key.
        let current = self.workspace(access, id).await?;
        if matches!(current.row["state"].as_str(), Some("deleted" | "deleting"))
            || current.row["desired_state"] == "deleted"
            || current.row["wrapped_transcript_key"] != workspace.row["wrapped_transcript_key"]
        {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        let wrapped = workspace.row["wrapped_transcript_key"]
            .as_str()
            .ok_or_else(invalid)?;
        let transcript_key = reader
            .master
            .open_transcript_key(&access.owner_id, id, wrapped)
            .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?;
        let mut result = if page {
            json!({"page":{"cursor":cursor,"beforeSequence":before,"ciphertextSha256":digest(ciphertext.as_bytes()),"ciphertext":ciphertext,"transcriptKey":transcript_key.as_str()}})
        } else {
            json!({"checkpoint":{"metadata":{"workspaceId":id,"sessionId":session,"runtimeGeneration":generation,"cursor":cursor,"objectKey":key,"ciphertextSha256":metadata["ciphertext_sha256"],"ciphertextBytes":metadata["ciphertext_bytes"],"createdAt":metadata["created_at"]},"ciphertext":ciphertext,"transcriptKey":transcript_key.as_str()}})
        };
        if native {
            if page {
                result["page"]["payloadFormat"] = json!(zuse_cloud_projection::FORMAT);
            } else {
                result["checkpoint"]["metadata"]["payloadFormat"] =
                    json!(zuse_cloud_projection::FORMAT);
            }
        }
        Ok(result)
    }
}
fn invalid() -> ApiError {
    ApiError::unavailable("cloud_transcript_metadata_invalid")
}
