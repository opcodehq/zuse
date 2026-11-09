//! Pin the admitting account and finish accepted uploads even if the client disconnects.
use super::*;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use sha2::{Digest, Sha256};
use zuse_protocol::cloud_assets as wire;

pub async fn create(
    auth: &Auth,
    uploads: crate::uploads::Uploads,
    mut params: Value,
) -> Result<Value> {
    let api = CloudApi::shared()?;
    ensure!(
        auth.workos_enabled() || api.loopback,
        "Sign in to access cloud workspaces."
    );
    let account = auth
        .user_id()
        .ok_or_else(|| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
    let organization = auth.state().org_id().map(str::to_owned);
    ensure!(
        params["expectedAccountId"].as_str() == Some(account.as_str())
            && params["expectedOrganizationId"].as_str() == organization.as_deref(),
        "The account changed. Select the cloud destination again."
    );
    let token = auth
        .access_token()
        .await
        .map_err(|_| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
    ensure!(
        auth.user_id().as_deref() == Some(account.as_str())
            && auth.state().org_id() == organization.as_deref(),
        "The account changed. Select the cloud destination again."
    );
    let transfers: Vec<crate::uploads::AttachmentTransfer> =
        serde_json::from_value(params["attachmentTransfers"].clone())?;
    ensure!(
        !transfers.is_empty() && transfers.len() <= wire::MAX_ASSETS,
        "Attach up to eight files to a cloud message."
    );
    params
        .as_object_mut()
        .unwrap()
        .remove("attachmentTransfers");
    params.as_object_mut().unwrap().remove("expectedAccountId");
    params
        .as_object_mut()
        .unwrap()
        .remove("expectedOrganizationId");
    let request: zuse_protocol::cloud::CloudWorkspaceCreateRequest =
        serde_json::from_value(params)?;
    ensure!(
        request.attachments.is_empty(),
        "Cloud attachment references cannot be combined with local uploads."
    );
    // The admitted task owns its token, file store and request. Dropping the RPC waiter
    // must not interrupt create after the authority has accepted the first message.
    tokio::spawn(async move {
        api.upload_and_create(
            &token,
            &account,
            organization.as_deref(),
            uploads,
            request,
            transfers,
        )
        .await
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!("Cloud launch was interrupted. Retry the same request to check acceptance.")
    })?
}
impl CloudApi {
    async fn upload_and_create(
        &self,
        token: &str,
        account: &str,
        organization: Option<&str>,
        uploads: crate::uploads::Uploads,
        mut request: zuse_protocol::cloud::CloudWorkspaceCreateRequest,
        transfers: Vec<crate::uploads::AttachmentTransfer>,
    ) -> Result<Value> {
        let mut seen = std::collections::HashSet::new();
        for transfer in transfers {
            ensure!(
                transfer.upload_id.len() <= 64
                    && !transfer.upload_id.is_empty()
                    && transfer
                        .upload_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                    && seen.insert(transfer.upload_id.clone()),
                "Invalid cloud attachment identity."
            );
            let source = crate::uploads::pending_ref(&transfer.upload_id, &transfer.file_name);
            let store = uploads.clone();
            let name = transfer.file_name.clone();
            let (bytes, mime) =
                tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, String)> {
                    let mut offset = 0;
                    let mut bytes = Vec::new();
                    let mime = launch_mime(&name)
                        .ok_or_else(|| {
                            anyhow::anyhow!("This attachment type is unavailable for cloud launch.")
                        })?
                        .to_owned();
                    loop {
                        let chunk = store.read_chunk(&source, offset, &[])?;
                        let part = STANDARD.decode(&chunk.data)?;
                        ensure!(
                            bytes.len() + part.len() <= wire::MAX_BYTES,
                            "Cloud attachments must be 20 MB or smaller."
                        );
                        bytes.extend_from_slice(&part);
                        if chunk.done {
                            break;
                        }
                        ensure!(
                            chunk.next_offset > offset,
                            "Attachment read did not advance."
                        );
                        offset = chunk.next_offset;
                    }
                    ensure!(!bytes.is_empty(), "Cloud attachments cannot be empty.");
                    Ok((bytes, mime))
                })
                .await??;
            for (seq, chunk) in bytes.chunks(wire::CHUNK_BYTES).enumerate() {
                self.request_scoped(
                    token,
                    &["v1", "cloud", "launch-assets", "chunk"],
                    Some(serde_json::to_value(wire::Chunk {
                        idempotency_key: request.idempotency_key.clone(),
                        upload_id: transfer.upload_id.clone(),
                        seq,
                        bytes: URL_SAFE_NO_PAD.encode(chunk),
                    })?),
                    organization,
                    &[],
                )
                .await?;
            }
            let result = self
                .request_scoped(
                    token,
                    &["v1", "cloud", "launch-assets", "commit"],
                    Some(serde_json::to_value(wire::Commit {
                        idempotency_key: request.idempotency_key.clone(),
                        upload_id: transfer.upload_id,
                        mime_type: mime,
                        original_name: transfer.file_name,
                        size_bytes: bytes.len(),
                        sha256: URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes)),
                    })?),
                    organization,
                    &[],
                )
                .await?;
            request
                .attachments
                .push(serde_json::from_value(result["asset"].clone())?);
        }
        let value = self
            .request_scoped(
                token,
                &["v1", "cloud", "workspaces"],
                Some(serde_json::to_value(request)?),
                organization,
                &[],
            )
            .await?;
        Ok(json!({"accountId":account,"organizationId":organization,"workspace":value}))
    }
}
fn launch_mime(name: &str) -> Option<&'static str> {
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
