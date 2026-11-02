//! Cached reads only: this transport has no gateway, resume or command APIs.
use super::*;
use std::time::Duration;

#[derive(Clone)]
pub struct Reader {
    http: reqwest::Client,
    origin: reqwest::Url,
    scope: Option<String>,
}
impl Reader {
    pub fn new(origin: &str) -> Result<Self> {
        let url = reqwest::Url::parse(origin)?;
        let loopback = url.host_str().is_some_and(|h| {
            h == "localhost" || h.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback())
        });
        ensure!(
            (url.scheme() == "https" || url.scheme() == "http" && loopback)
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none(),
            "invalid_cloud_api_origin"
        );
        Ok(Self {
            http: {
                let builder = reqwest::Client::builder();
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.redirect(reqwest::redirect::Policy::none());
                builder.build()?
            },
            origin: url,
            scope: None,
        })
    }
    pub fn with_scope(mut self, scope: &str) -> Result<Self> {
        if scope == "personal" {
            self.scope = None;
            return Ok(self);
        }
        let id = scope
            .strip_prefix("organization:")
            .ok_or_else(|| anyhow::anyhow!("invalid_workspace_scope"))?;
        ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "invalid_workspace_scope"
        );
        self.scope = Some(scope.into());
        Ok(self)
    }
    async fn get(
        &self,
        token: &str,
        workspace: &str,
        session: &str,
        page: Option<(&Cursor, i64)>,
    ) -> Result<serde_json::Value> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid_cloud_api_origin"))?
            .clear()
            .extend([
                "v1",
                "cloud",
                "workspaces",
                workspace,
                "sessions",
                session,
                if page.is_some() {
                    "transcript-message-page"
                } else {
                    "transcript-checkpoint"
                },
            ]);
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("format", FORMAT);
            if let Some((cursor, before)) = page {
                query
                    .append_pair("epoch", &cursor.epoch)
                    .append_pair("version", &cursor.version.to_string())
                    .append_pair("beforeSequence", &before.to_string());
            }
        }
        let requested = url.clone();
        let request = self
            .http
            .get(url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(10));
        let request = if let Some(scope) = &self.scope {
            request.header("x-zuse-workspace", scope)
        } else {
            request
        };
        #[cfg(target_arch = "wasm32")]
        let request = request.fetch_credentials_omit().fetch_cache_no_store();
        let response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("cloud_projection_unavailable"))?;
        ensure!(
            response.url() == &requested,
            "cloud_projection_redirect_rejected"
        );
        ensure!(
            response.status().is_success(),
            "cloud_projection_read_failed"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= (2 * PAGE_LIMIT) as u64),
            "cloud_projection_too_large"
        );
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream
            .next()
            .await
            .transpose()
            .map_err(|_| anyhow::anyhow!("cloud_projection_unavailable"))?
        {
            ensure!(
                bytes.len() + chunk.len() <= 2 * PAGE_LIMIT,
                "cloud_projection_too_large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub async fn head(&self, token: &str, workspace: &str, session: &str) -> Result<Option<Head>> {
        let response = self.get(token, workspace, session, None).await?;
        let access = &response["checkpoint"];
        if access.is_null() {
            return Ok(None);
        }
        let metadata = &access["metadata"];
        ensure!(
            metadata["payloadFormat"] == FORMAT
                && metadata["workspaceId"] == workspace
                && metadata["sessionId"] == session,
            "cloud_projection_scope_mismatch"
        );
        let cursor: Cursor = serde_json::from_value(metadata["cursor"].clone())?;
        let ciphertext = access["ciphertext"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection"))?;
        ensure!(
            metadata["ciphertextSha256"] == digest(ciphertext.as_bytes())
                && metadata["ciphertextBytes"].as_u64() == Some(ciphertext.len() as u64),
            "cloud_projection_integrity_failed"
        );
        let key = TranscriptKey::parse(
            access["transcriptKey"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection_key"))?,
        )?;
        let head: Head = key.open(workspace, session, &cursor, None, ciphertext)?;
        ensure!(
            head.schema_version == 1
                && head.payload_format == FORMAT
                && head.workspace_id == workspace
                && head.session_id == session
                && head.chat.id == session
                && head.cursor == cursor
                && metadata["runtimeGeneration"].as_i64() == Some(head.runtime_generation)
                && matches!(head.transcript.frame, TranscriptFrame::Reset { .. }),
            "cloud_projection_scope_mismatch"
        );
        Ok(Some(head))
    }
    pub async fn page(&self, token: &str, head: &Head, before: i64) -> Result<Option<Page>> {
        ensure!(before > 0, "invalid_cloud_projection_page");
        let response = self
            .get(
                token,
                &head.workspace_id,
                &head.session_id,
                Some((&head.cursor, before)),
            )
            .await?;
        let access = &response["page"];
        if access.is_null() {
            return Ok(None);
        }
        ensure!(
            access["payloadFormat"] == FORMAT && access["beforeSequence"].as_i64() == Some(before),
            "cloud_projection_scope_mismatch"
        );
        let ciphertext = access["ciphertext"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection"))?;
        ensure!(
            access["ciphertextSha256"] == digest(ciphertext.as_bytes()),
            "cloud_projection_integrity_failed"
        );
        let key = TranscriptKey::parse(
            access["transcriptKey"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection_key"))?,
        )?;
        let page: Page = key.open(
            &head.workspace_id,
            &head.session_id,
            &head.cursor,
            Some(before),
            ciphertext,
        )?;
        ensure!(
            page.schema_version == 1
                && page.payload_format == FORMAT
                && page.workspace_id == head.workspace_id
                && page.session_id == head.session_id
                && page.runtime_generation == head.runtime_generation
                && page.cursor == head.cursor
                && page.before_sequence == before
                && page.older_message_sequence.unwrap_or(0) >= 0
                && page.older_message_sequence.unwrap_or(0) + (page.entries.len() as i64) == before,
            "cloud_projection_scope_mismatch"
        );
        Ok(Some(page))
    }
}
