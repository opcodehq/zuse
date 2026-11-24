//! API-owned launch assets. Content is uploaded before workspace allocation.
use serde::{Deserialize, Serialize};
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ASSETS: usize = 8;
pub const CHUNK_BYTES: usize = 512 * 1024;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub asset_id: String,
    pub mime_type: String,
    pub original_name: String,
    pub size_bytes: usize,
    #[serde(default)]
    pub sha256: String,
}
/// Version of [`PendingAsset`] launches a server accepts (its context's
/// `launchAttachmentHoldVersion`).
pub const LAUNCH_ATTACHMENT_HOLD_VERSION: u32 = 1;
/// How long a new chat's first turn waits for files still uploading. Past
/// this the turn goes to the computer, which reports the missing file.
pub const PENDING_ASSET_WAIT_MS: i64 = 15 * 60 * 1000;
/// A first-message file declared before its bytes arrive. The server derives
/// its asset id from `upload_id`; the bytes then upload to the new computer's
/// asset storage under the same id.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingAsset {
    pub upload_id: String,
    pub mime_type: String,
    pub original_name: String,
    pub size_bytes: usize,
    pub sha256: String,
}
/// The file name a commit records: the last path segment, trimmed.
pub fn committed_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or("").trim()
}
/// The MIME type cloud storage records for a file name's extension.
pub fn mime_for_name(name: &str) -> Option<&'static str> {
    match std::path::Path::new(name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "pdf" => Some("application/pdf"),
        "txt" => Some("text/plain"),
        "md" | "markdown" => Some("text/markdown"),
        "json" => Some("application/json"),
        "zip" => Some("application/zip"),
        _ => None,
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub idempotency_key: String,
    pub upload_id: String,
    pub seq: usize,
    pub bytes: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub idempotency_key: String,
    pub upload_id: String,
    pub mime_type: String,
    pub original_name: String,
    pub size_bytes: usize,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub asset: Asset,
    pub bytes: String,
}
pub fn supported_mime(value: &str) -> bool {
    matches!(
        value,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "application/pdf"
            | "text/plain"
            | "text/markdown"
            | "application/json"
            | "application/zip"
    )
}
