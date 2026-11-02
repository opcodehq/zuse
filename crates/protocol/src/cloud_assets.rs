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
