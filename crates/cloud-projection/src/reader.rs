//! Cached reads only: this transport has no gateway, resume or command APIs.
use super::*;
use std::time::Duration;

#[derive(Clone)]
pub struct Reader {
    http: reqwest::Client,
    origin: reqwest::Url,
    scope: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    cache: Option<std::sync::Arc<crate::cache::PageCache>>,
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
            #[cfg(not(target_arch = "wasm32"))]
            cache: None,
        })
    }
    /// Account-scoped, ciphertext-only storage. Authorization still comes from
    /// a fresh head; no token, transcript key or plaintext is persisted.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_page_cache(&self, root: &std::path::Path, account: &str) -> Result<Self> {
        let mut reader = self.clone();
        reader.cache = Some(crate::cache::PageCache::shared(
            root,
            self.origin.as_str(),
            self.scope.as_deref(),
            account,
        )?);
        Ok(reader)
    }
    pub fn with_scope(mut self, scope: &str) -> Result<Self> {
        // A cache authorized for one scope must not follow a scope change.
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.cache = None;
        }
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn clear_native_cache(root: &std::path::Path) -> Result<()> {
        crate::cache::PageCache::purge_root(root)
    }
    /// Forget one workspace's cached history, keeping every other chat.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn forget_native_workspace(root: &std::path::Path, workspace: &str) -> Result<()> {
        crate::cache::PageCache::forget_workspace(root, workspace)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn cached_head(&self, token: &str, workspace: &str, session: &str) -> Result<Option<Head>> {
        self.cache
            .as_ref()
            .map(|cache| cache.cached_head(workspace, session, token))
            .unwrap_or(Ok(None))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn provisional_head(&self, workspace: &str, session: Option<&str>) -> Result<Option<Head>> {
        self.cache
            .as_ref()
            .map(|cache| cache.provisional_head(workspace, session))
            .unwrap_or(Ok(None))
    }
    /// Persist a server-authorized PG projection without exposing its data key
    /// to the viewport. This is the same envelope contract as native R2 history.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn cache_projection(&self, token: &str, projection: &serde_json::Value) -> Result<()> {
        let key = projection["transcriptKey"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection_key"))?;
        let ciphertext = projection["ciphertext"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection"))?;
        let workspace = projection["workspaceId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection"))?;
        let session = projection["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection"))?;
        let cursor: Cursor = serde_json::from_value(projection["cursor"].clone())?;
        let before = projection["beforeSequence"].as_i64();
        let key_object = TranscriptKey::parse(key)?;
        if let Some(before) = before {
            let page: Page =
                key_object.open(workspace, session, &cursor, Some(before), ciphertext)?;
            ensure!(
                page.workspace_id == workspace
                    && page.session_id == session
                    && page.cursor == cursor
                    && page.before_sequence == before,
                "cloud_projection_scope_mismatch"
            );
            if let Some(cache) = &self.cache {
                if let Some(head) = cache.cached_head(workspace, session, token)? {
                    ensure!(
                        head.cursor == cursor && head.runtime_generation == page.runtime_generation,
                        "cloud_projection_scope_mismatch"
                    );
                    let _ = cache.write(&head, before, ciphertext);
                }
            }
        } else {
            let head: Head = key_object.open(workspace, session, &cursor, None, ciphertext)?;
            ensure!(
                head.workspace_id == workspace
                    && head.session_id == session
                    && head.cursor == cursor,
                "cloud_projection_scope_mismatch"
            );
            if let Some(cache) = &self.cache {
                cache.authorize(&head, token, key);
                let _ = cache.write_head(&head, ciphertext);
            }
        }
        Ok(())
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
        // A 401 is an expired bearer the next refresh replaces; it says
        // nothing about access. A 403 withdraws this workspace only.
        #[cfg(not(target_arch = "wasm32"))]
        if response.status().as_u16() == 403
            && let Some(cache) = &self.cache
        {
            let _ = crate::cache::PageCache::forget_workspace(cache.root(), workspace);
        }
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
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(cache) = &self.cache {
            cache.authorize(&head, token, access["transcriptKey"].as_str().unwrap());
            let _ = cache.write_head(&head, ciphertext);
        }
        Ok(Some(head))
    }
    pub async fn page(&self, token: &str, head: &Head, before: i64) -> Result<Option<Page>> {
        ensure!(before > 0, "invalid_cloud_projection_page");
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(cache) = &self.cache {
            if let Ok(Some((ciphertext, key))) = cache.read(head, before, token) {
                if let Ok(page) = Self::open_page(head, before, &key, &ciphertext) {
                    return Ok(Some(page));
                }
            }
        }
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
        let encoded_key = access["transcriptKey"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid_cloud_projection_key"))?;
        let page = Self::open_page(head, before, encoded_key, ciphertext)?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(cache) = &self.cache {
            // A full/unavailable cache must not prevent an authorized network read.
            let _ = cache.write(head, before, ciphertext);
        }
        Ok(Some(page))
    }

    fn open_page(head: &Head, before: i64, encoded_key: &str, ciphertext: &str) -> Result<Page> {
        let key = TranscriptKey::parse(encoded_key)?;
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
                && page.older_message_sequence.unwrap_or(0) < before
                && page.older_message_sequence.unwrap_or(0) + (page.entries.len() as i64) == before,
            "cloud_projection_scope_mismatch"
        );
        Ok(page)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod cache_tests {
    use super::*;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn fixtures() -> (Head, serde_json::Value, serde_json::Value) {
        let encoded = B64.encode([42; 32]);
        let key = TranscriptKey::parse(&encoded).unwrap();
        let chat = serde_json::from_value(json!({"id":"session","deviceId":"device","title":"Private history","archived":true,"cwd":"/repo","branch":"feature","checkoutId":null,"config":null,"lastMessagePreview":null,"lastMessageAt":null,"createdAt":"2026-10-23T00:00:00Z"})).unwrap();
        let head = Head {
            slack_results: Default::default(),
            schema_version: 1,
            payload_format: FORMAT.into(),
            workspace_id: "workspace".into(),
            session_id: "session".into(),
            runtime_generation: 2,
            cursor: Cursor {
                epoch: "epoch".into(),
                version: 1,
            },
            chat,
            conversations: vec![],
            transcript: zuse_model::TranscriptUpdate {
                frame: TranscriptFrame::Reset {
                    reset: vec![].into(),
                },
                context_usage: None,
                replay_baseline: None,
            },
            queue: json!([]),
            queue_paused: true,
            older_message_sequence: Some(1),
        };
        let page = Page {
            schema_version: 1,
            payload_format: FORMAT.into(),
            workspace_id: head.workspace_id.clone(),
            session_id: head.session_id.clone(),
            runtime_generation: 2,
            cursor: head.cursor.clone(),
            before_sequence: 1,
            older_message_sequence: None,
            entries: vec![zuse_model::SessionMessageEntry {
                id: "message".into(),
                role: zuse_model::MessageRole::Assistant,
                parts: vec![zuse_model::MessagePart::Text {
                    id: "part".into(),
                    text: "private cached text".into(),
                }],
                created_at: 1,
                device_id: "device".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            }],
        };
        let upload = key
            .seal("workspace", "session", &head.cursor, None, &head)
            .unwrap();
        let checkpoint = json!({"checkpoint":{"metadata":{"payloadFormat":FORMAT,"workspaceId":"workspace","sessionId":"session","runtimeGeneration":2,"cursor":head.cursor,"ciphertextSha256":upload.ciphertext_sha256,"ciphertextBytes":upload.ciphertext.len()},"ciphertext":upload.ciphertext,"transcriptKey":encoded}});
        let upload = key
            .seal("workspace", "session", &head.cursor, Some(1), &page)
            .unwrap();
        let page = json!({"page":{"payloadFormat":FORMAT,"beforeSequence":1,"ciphertext":upload.ciphertext,"ciphertextSha256":upload.ciphertext_sha256,"transcriptKey":encoded}});
        (head, checkpoint, page)
    }

    #[tokio::test]
    async fn persisted_pages_skip_network_after_authorized_reopen_and_repair_corruption() {
        let (head, checkpoint, page) = fixtures();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let mut buffer = [0; 2048];
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..n]);
                }
                count.fetch_add(1, Ordering::SeqCst);
                let is_page = String::from_utf8(request)
                    .unwrap()
                    .contains("/transcript-message-page?");
                let body = if is_page { &page } else { &checkpoint }.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let reader = Reader::new(&origin)
            .unwrap()
            .with_page_cache(root.path(), "account")
            .unwrap();
        let authorized = reader
            .head("token", "workspace", "session")
            .await
            .unwrap()
            .unwrap();
        assert!(
            reader
                .cached_head("token", "workspace", "session")
                .unwrap()
                .is_some()
        );
        let first = reader.page("token", &authorized, 1).await.unwrap().unwrap();
        assert_eq!(first.entries.len(), 1);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        drop(reader);

        let reader = Reader::new(&origin)
            .unwrap()
            .with_page_cache(root.path(), "account")
            .unwrap();
        assert!(
            reader
                .cache
                .as_ref()
                .unwrap()
                .read(&head, 1, "token")
                .unwrap()
                .is_some()
        );
        // Reopening a reader in this process retains only the same fresh token
        // grant. A new process has ciphertext but no authorized in-memory key.
        let unshared =
            crate::cache::PageCache::open(root.path(), reader.origin.as_str(), None, "account")
                .unwrap();
        assert!(unshared.read(&head, 1, "token").unwrap().is_none());
        #[cfg(target_os = "macos")]
        {
            // A separately opened cache has no in-memory authorization grant.
            // Only a provisional head can use its Keychain-held transcript key;
            // history pages still require fresh account authorization.
            let provisional = unshared
                .provisional_head("workspace", Some("session"))
                .unwrap()
                .expect("macOS Keychain must reopen the encrypted provisional head");
            assert_eq!(provisional.workspace_id, head.workspace_id);
            assert_eq!(provisional.session_id, head.session_id);
            assert_eq!(provisional.cursor, head.cursor);
            assert!(
                unshared
                    .provisional_head("other-workspace", Some("session"))
                    .unwrap()
                    .is_none()
            );
            assert!(unshared.read(&head, 1, "token").unwrap().is_none());
        }
        let authorized = reader
            .head("token", "workspace", "session")
            .await
            .unwrap()
            .unwrap();
        assert!(
            reader
                .page("token", &authorized, 1)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(requests.load(Ordering::SeqCst), 3); // Only the head was fetched again.
        assert!(
            reader
                .cache
                .as_ref()
                .unwrap()
                .read(&head, 1, "other-token")
                .unwrap()
                .is_none()
        );
        let other = Reader::new(&origin)
            .unwrap()
            .with_page_cache(root.path(), "other-account")
            .unwrap();
        other
            .cache
            .as_ref()
            .unwrap()
            .authorize(&head, "token", &B64.encode([42; 32]));
        assert!(
            other
                .cache
                .as_ref()
                .unwrap()
                .read(&head, 1, "token")
                .unwrap()
                .is_none()
        );
        for (cache_origin, cache_scope) in [
            ("https://other.example", None),
            (origin.as_str(), Some("organization:other")),
        ] {
            let cache =
                crate::cache::PageCache::open(root.path(), cache_origin, cache_scope, "account")
                    .unwrap();
            cache.authorize(&head, "token", &B64.encode([42; 32]));
            assert!(cache.read(&head, 1, "token").unwrap().is_none());
        }
        let mut next = head.clone();
        next.cursor.version += 1;
        let cache = reader.cache.as_ref().unwrap();
        cache.authorize(&next, "token", &B64.encode([42; 32]));
        assert!(cache.read(&next, 1, "token").unwrap().is_none());
        next = head.clone();
        next.runtime_generation += 1;
        cache.authorize(&next, "token", &B64.encode([42; 32]));
        assert!(cache.read(&next, 1, "token").unwrap().is_none());
        let db = rusqlite::Connection::open(root.path().join("encrypted-pages.sqlite3")).unwrap();
        let stored: String = db
            .query_row(
                "SELECT ciphertext FROM encrypted_pages LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.contains("private cached text"));
        let columns: Vec<String> = db
            .prepare("PRAGMA table_info(encrypted_pages)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(
            columns,
            ["namespace", "identity", "ciphertext", "digest", "workspace"]
        );
        db.execute("UPDATE encrypted_pages SET ciphertext='damaged'", [])
            .unwrap();
        assert!(
            reader
                .page("token", &authorized, 1)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(requests.load(Ordering::SeqCst), 4);
        server.abort();
    }

    #[test]
    fn pg_opening_heads_and_pages_are_encrypted_scoped_bounded_and_purged() {
        let (head, checkpoint, page) = fixtures();
        let root = tempfile::tempdir().unwrap();
        let reader = Reader::new("https://staging.example")
            .unwrap()
            .with_page_cache(root.path(), "account-a")
            .unwrap();
        let projection = json!({"workspaceId":"workspace","sessionId":"session","cursor":head.cursor,"beforeSequence":null,"ciphertext":checkpoint["checkpoint"]["ciphertext"],"transcriptKey":checkpoint["checkpoint"]["transcriptKey"]});
        reader.cache_projection("token-a", &projection).unwrap();
        assert!(
            reader
                .cached_head("token-a", "workspace", "session")
                .unwrap()
                .is_some()
        );
        assert!(
            reader
                .cached_head("other-token", "workspace", "session")
                .unwrap()
                .is_none()
        );
        let projection = json!({"workspaceId":"workspace","sessionId":"session","cursor":head.cursor,"beforeSequence":1,"ciphertext":page["page"]["ciphertext"],"transcriptKey":page["page"]["transcriptKey"]});
        reader.cache_projection("token-a", &projection).unwrap();
        assert!(
            reader
                .cache
                .as_ref()
                .unwrap()
                .read(&head, 1, "token-a")
                .unwrap()
                .is_some()
        );
        for (origin, account, scope) in [
            ("https://production.example", "account-a", "personal"),
            ("https://staging.example", "account-b", "personal"),
            ("https://staging.example", "account-a", "organization:org"),
        ] {
            let other = Reader::new(origin)
                .unwrap()
                .with_scope(scope)
                .unwrap()
                .with_page_cache(root.path(), account)
                .unwrap();
            assert!(
                other
                    .cached_head("token-a", "workspace", "session")
                    .unwrap()
                    .is_none()
            );
        }
        let bytes = std::fs::read(root.path().join("encrypted-pages.sqlite3")).unwrap();
        assert!(
            !bytes
                .windows(b"Private history".len())
                .any(|window| window == b"Private history")
        );
        assert!(
            !bytes
                .windows(b"private cached text".len())
                .any(|window| window == b"private cached text")
        );
        Reader::clear_native_cache(root.path()).unwrap();
        assert!(
            reader
                .cached_head("token-a", "workspace", "session")
                .unwrap()
                .is_none()
        );
        assert!(
            reader
                .cache
                .as_ref()
                .unwrap()
                .read(&head, 1, "token-a")
                .unwrap()
                .is_none()
        );
        let db = rusqlite::Connection::open(root.path().join("encrypted-pages.sqlite3")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM encrypted_pages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for version in 0..300 {
            let mut head = head.clone();
            head.cursor.version = version;
            reader
                .cache
                .as_ref()
                .unwrap()
                .write(&head, 1, "ciphertext-only")
                .unwrap();
        }
        assert!(
            db.query_row("SELECT count(*) FROM encrypted_pages", [], |row| row
                .get::<_, i64>(0))
                .unwrap()
                <= 256
        );
    }

    /// What a restarted app sees: a new cache instance with no in-memory
    /// leases still opens the last saved head. Forgetting one workspace (it
    /// was deleted, or access to it was withdrawn) leaves every other chat.
    #[test]
    fn saved_heads_paint_after_restart_and_forgetting_one_workspace_keeps_the_rest() {
        let (head, checkpoint, _) = fixtures();
        let root = tempfile::tempdir().unwrap();
        let reader = Reader::new("https://staging.example")
            .unwrap()
            .with_page_cache(root.path(), "account-a")
            .unwrap();
        let origin = reader.origin.as_str();
        let encoded = B64.encode([42; 32]);
        let key = TranscriptKey::parse(&encoded).unwrap();
        let projection = json!({"workspaceId":"workspace","sessionId":"session","cursor":head.cursor,"beforeSequence":null,"ciphertext":checkpoint["checkpoint"]["ciphertext"],"transcriptKey":encoded});
        reader.cache_projection("token-a", &projection).unwrap();
        let mut other = head.clone();
        other.workspace_id = "other".into();
        let sealed = key
            .seal("other", "session", &other.cursor, None, &other)
            .unwrap();
        let projection = json!({"workspaceId":"other","sessionId":"session","cursor":other.cursor,"beforeSequence":null,"ciphertext":sealed.ciphertext,"transcriptKey":encoded});
        reader.cache_projection("token-a", &projection).unwrap();

        let restarted =
            crate::cache::PageCache::open(root.path(), origin, None, "account-a").unwrap();
        let painted = restarted
            .provisional_head("workspace", Some("session"))
            .unwrap()
            .expect("a restarted app paints the saved head");
        assert_eq!(painted.chat.title.as_deref(), Some("Private history"));
        let stranger =
            crate::cache::PageCache::open(root.path(), origin, None, "account-b").unwrap();
        assert!(
            stranger
                .provisional_head("workspace", Some("session"))
                .unwrap()
                .is_none(),
            "another account never sees this account's history"
        );

        Reader::forget_native_workspace(root.path(), "workspace").unwrap();
        let restarted =
            crate::cache::PageCache::open(root.path(), origin, None, "account-a").unwrap();
        assert!(
            restarted
                .provisional_head("workspace", Some("session"))
                .unwrap()
                .is_none()
        );
        assert!(
            restarted
                .provisional_head("other", Some("session"))
                .unwrap()
                .is_some(),
            "forgetting one workspace keeps the others"
        );
        assert!(
            reader
                .cached_head("token-a", "workspace", "session")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    #[ignore = "subprocess fixture, invoked by native_ciphertext_survives_a_fresh_process"]
    async fn cache_fresh_process_helper() {
        let spec: serde_json::Value =
            serde_json::from_str(&std::env::var("ZUSE_TRANSCRIPT_CACHE_TEST").unwrap()).unwrap();
        let reader = Reader::new(spec["origin"].as_str().unwrap())
            .unwrap()
            .with_page_cache(
                std::path::Path::new(spec["root"].as_str().unwrap()),
                "account",
            )
            .unwrap();
        let (old_head, _, _) = fixtures();
        assert!(
            reader
                .cached_head("token", "workspace", "session")
                .unwrap()
                .is_none()
        );
        assert!(
            reader
                .cache
                .as_ref()
                .unwrap()
                .read(&old_head, 1, "token")
                .unwrap()
                .is_none(),
            "Ciphertext on disk must not create an authorization grant in a new process"
        );
        #[cfg(not(target_os = "macos"))]
        assert!(
            reader
                .provisional_head("workspace", Some("session"))
                .unwrap()
                .is_none()
        );
        let head = reader
            .head("token", "workspace", "session")
            .await
            .unwrap()
            .unwrap();
        let page = reader.page("token", &head, 1).await.unwrap().unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].id, "message");
    }

    #[tokio::test]
    async fn native_ciphertext_survives_a_fresh_process() {
        let (_, checkpoint, page) = fixtures();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                    let mut buffer = [0; 2048];
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..n]);
                }
                count.fetch_add(1, Ordering::SeqCst);
                let is_page = String::from_utf8(request)
                    .unwrap()
                    .contains("/transcript-message-page?");
                let body = if is_page { &page } else { &checkpoint }.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let reader = Reader::new(&origin)
            .unwrap()
            .with_page_cache(root.path(), "account")
            .unwrap();
        let head = reader
            .head("token", "workspace", "session")
            .await
            .unwrap()
            .unwrap();
        reader.page("token", &head, 1).await.unwrap().unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        drop(reader);
        let fresh_device = tempfile::tempdir().unwrap();
        for (profile, expected_reads) in [(root.path(), 3), (fresh_device.path(), 5)] {
            let spec = json!({"origin":origin,"root":profile}).to_string();
            let status = tokio::task::spawn_blocking(move || {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("ZUSE_TRANSCRIPT_CACHE_TEST", spec)
                    .args([
                        "--ignored",
                        "--exact",
                        "reader::cache_tests::cache_fresh_process_helper",
                        "--nocapture",
                    ])
                    .status()
                    .unwrap()
            })
            .await
            .unwrap();
            assert!(status.success());
            assert_eq!(
                requests.load(Ordering::SeqCst),
                expected_reads,
                "Retained profile needs only fresh head authorization; a new profile also fetches the page"
            );
        }
        let encoded_key = B64.encode([42; 32]);
        for entry in std::fs::read_dir(root.path()).unwrap() {
            let path = entry.unwrap().path();
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(path).unwrap();
            for secret in [b"private cached text".as_slice(), encoded_key.as_bytes()] {
                assert!(
                    !bytes.windows(secret.len()).any(|part| part == secret),
                    "Native cache persisted plaintext or a transcript key"
                );
            }
        }
        server.abort();
    }
}
