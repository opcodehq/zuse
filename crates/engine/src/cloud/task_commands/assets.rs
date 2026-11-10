//! Bounded temporary attachment bytes, scoped to actor/organization/computer.
//! Never written to config, transcript, disk cache or guest RPC.
use super::*;
use base64::engine::general_purpose::STANDARD;
use std::{collections::BTreeMap, sync::Mutex};
use zeroize::Zeroizing;
type Key = (String, Option<String>, String, String);
#[derive(Default)]
pub(super) struct Manager(
    Mutex<std::collections::HashMap<Key, Upload>>,
    Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
);
struct Upload {
    parts: BTreeMap<usize, Zeroizing<Vec<u8>>>,
    at: i64,
    shared_budget: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
    reserved: usize,
}
impl Drop for Upload {
    fn drop(&mut self) {
        if let Some(budget) = &self.shared_budget {
            budget.fetch_sub(self.reserved, std::sync::atomic::Ordering::AcqRel);
        }
    }
}
fn key(account: &str, organization: Option<&str>, workspace: &str, upload: &str) -> Key {
    (
        account.into(),
        organization.map(str::to_owned),
        workspace.into(),
        upload.into(),
    )
}
impl Manager {
    pub(super) fn hosted() -> Self {
        static BUDGET: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicUsize>> =
            std::sync::OnceLock::new();
        Self(
            Mutex::default(),
            Some(
                BUDGET
                    .get_or_init(|| std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)))
                    .clone(),
            ),
        )
    }

    fn part(&self, k: Key, seq: usize, bytes: Vec<u8>) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut all = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Cloud upload staging is unavailable."))?;
        all.retain(|scope, u| scope.0 == k.0 && scope.1 == k.1 && u.at > now - 600_000);
        ensure!(
            seq < 4096 && bytes.len() <= zuse_protocol::cloud_assets::CHUNK_BYTES,
            "Cloud upload chunk exceeds its bound."
        );
        if let Some(old) = all.get(&k).and_then(|u| u.parts.get(&seq)) {
            ensure!(old.as_slice() == bytes, "Cloud upload identity changed.");
            return Ok(());
        }
        let total: usize = all
            .values()
            .flat_map(|u| u.parts.values())
            .map(|p| p.len())
            .sum();
        ensure!(
            all.contains_key(&k) || all.len() < 32,
            "Too many unfinished cloud uploads."
        );
        ensure!(
            total + bytes.len() <= 64 * 1024 * 1024,
            "Cloud uploads reached their temporary memory limit."
        );
        let u = all.entry(k).or_insert_with(|| Upload {
            parts: BTreeMap::new(),
            at: now,
            shared_budget: self.1.clone(),
            reserved: 0,
        });
        ensure!(
            u.parts.values().map(|p| p.len()).sum::<usize>() + bytes.len()
                <= zuse_protocol::cloud_assets::MAX_BYTES,
            "Cloud attachments must be20MB or smaller."
        );
        if let Some(budget) = &u.shared_budget {
            ensure!(
                budget
                    .fetch_update(
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                        |used| used
                            .checked_add(bytes.len())
                            .filter(|n| *n <= 64 * 1024 * 1024)
                    )
                    .is_ok(),
                "Hosted browser uploads reached their shared memory limit. Finish or cancel an earlier upload."
            );
            u.reserved += bytes.len();
        }
        u.parts.insert(seq, Zeroizing::new(bytes));
        u.at = now;
        Ok(())
    }
    fn assembled(&self, k: &Key) -> Result<Zeroizing<Vec<u8>>> {
        let all = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Cloud upload staging is unavailable."))?;
        let u = all
            .get(k)
            .filter(|u| u.at > chrono::Utc::now().timestamp_millis() - 600_000)
            .ok_or_else(|| anyhow::anyhow!("Attachment staging expired. Attach the file again."))?;
        let mut bytes = Zeroizing::new(Vec::new());
        for (expected, (seq, part)) in u.parts.iter().enumerate() {
            ensure!(expected == *seq, "Attachment has missing chunks.");
            bytes.extend_from_slice(part);
        }
        ensure!(!bytes.is_empty(), "Cloud attachments cannot be empty.");
        Ok(bytes)
    }
    fn remove(&self, k: &Key) {
        if let Ok(mut all) = self.0.lock() {
            all.remove(k);
        }
    }
    pub(super) fn revoke(&self, account: &str, organization: Option<&str>, workspace: &str) {
        if let Ok(mut all) = self.0.lock() {
            all.retain(|k, _| {
                !(k.0 == account && k.1.as_deref() == organization && k.2 == workspace)
            });
        }
    }
}
fn mime(name: &str) -> Result<&'static str> {
    Ok(
        match std::path::Path::new(name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "avif" => "image/avif",
            "pdf" => "application/pdf",
            "txt" => "text/plain",
            "md" | "markdown" => "text/markdown",
            "json" => "application/json",
            "zip" => "application/zip",
            _ => anyhow::bail!("This attachment type is unavailable for cloud tasks."),
        },
    )
}
pub(in crate::cloud) async fn forward(
    api: &CloudApi,
    auth: &Auth,
    account: &str,
    organization: Option<&str>,
    workspace: &str,
    name: &str,
    p: Value,
) -> Result<Value> {
    scoped(auth, account, organization)?;
    let token = auth.access_token().await?;
    scoped(auth, account, organization)?;
    if name == methods::READ_ATTACHMENT_CHUNK {
        let asset = parse_asset_ref(p["path"].as_str().unwrap_or_default())?;
        let value = api
            .request_scoped(
                &token,
                &[
                    "v1",
                    "cloud",
                    "workspaces",
                    workspace,
                    "assets",
                    &asset.asset_id,
                    "chunks",
                ],
                None,
                organization,
                &[(
                    "offset".into(),
                    p["offset"].as_u64().unwrap_or(0).to_string(),
                )],
            )
            .await?;
        scoped(auth, account, organization)?;
        let actual: Asset = serde_json::from_value(value["asset"].clone())?;
        ensure!(asset == actual, "Cloud attachment binding changed.");
        return Ok(value);
    }
    let upload = string(&p, "uploadId")?;
    ensure!(
        upload.len() <= 64
            && upload
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
        "Invalid cloud upload identity."
    );
    let k = key(account, organization, workspace, &upload);
    if name == methods::UPLOAD_CHUNK {
        let seq = p["seq"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| anyhow::anyhow!("Cloud upload sequence is missing."))?;
        let data = p["data"]
            .as_str()
            .filter(|s| s.len() <= zuse_protocol::cloud_assets::CHUNK_BYTES * 4 / 3 + 8)
            .ok_or_else(|| anyhow::anyhow!("Invalid cloud upload chunk."))?;
        let bytes = STANDARD.decode(data)?;
        let encoded = URL_SAFE_NO_PAD.encode(&bytes);
        api.task_commands.uploads.part(k, seq, bytes)?;
        let result=api.request_scoped(&token,&["v1","cloud","workspaces",workspace,"assets","chunks"],Some(json!({"idempotencyKey":format!("workspace:{workspace}"),"uploadId":upload,"seq":seq,"bytes":encoded})),organization,&[]).await;
        scoped(auth, account, organization)?;
        return result;
    }
    ensure!(
        name == methods::UPLOAD_COMMIT,
        "Unsupported cloud asset action."
    );
    let file_name = string(&p, "fileName")?;
    let bytes = api.task_commands.uploads.assembled(&k)?;
    let result=api.request_scoped(&token,&["v1","cloud","workspaces",workspace,"assets","commit"],Some(json!({"idempotencyKey":format!("workspace:{workspace}"),"uploadId":upload,"mimeType":mime(&file_name)?,"originalName":file_name,"sizeBytes":bytes.len(),"sha256":URL_SAFE_NO_PAD.encode(Sha256::digest(bytes.as_slice()))})),organization,&[]).await?;
    scoped(auth, account, organization)?;
    let asset: Asset = serde_json::from_value(result["asset"].clone())?;
    ensure!(
        asset.size_bytes == bytes.len()
            && asset.sha256 == URL_SAFE_NO_PAD.encode(Sha256::digest(bytes.as_slice())),
        "Cloud upload receipt does not match its bytes."
    );
    let path = asset_ref(&asset)?;
    api.task_commands.uploads.remove(&k);
    Ok(json!({"path":path,"asset":asset,"name":asset.original_name,"mimeType":asset.mime_type}))
}
