//! Real HTTP/controller status tests run in a subprocess so cache-root overrides
//! cannot affect another test or the operator's native profile.
use super::*;

struct Controller {
    api: CloudApi,
    auth: Auth,
}
#[async_trait::async_trait]
impl zuse_rpc::RpcService for Controller {
    async fn handle(
        &self,
        method: &str,
        params: Value,
    ) -> std::result::Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
        let value = self
            .api
            .handle(&self.auth, method, params)
            .await
            .map_err(rpc_error)?;
        zuse_rpc::RpcReply::value(&value)
    }
}

#[test]
fn authoritative_workspace_missing_revokes_cache_but_missing_history_and_tools_do_not() {
    const CHILD: &str = "ZUSE_TEST_WORKSPACE_MISSING_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let keys = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .env("ZUSE_SECURE_CACHE_DIR", keys.path())
            .args(["--exact", "cloud::workspace_missing_tests::authoritative_workspace_missing_revokes_cache_but_missing_history_and_tools_do_not", "--nocapture"])
            .env(CHILD, "1")
            .env("ZUSE_CLOUD_HISTORY_CACHE_ROOT", root.path())
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(http_matrix());
}

async fn http_matrix() {
    use axum::{
        Router,
        extract::{OriginalUri, State},
        http::StatusCode,
        response::IntoResponse,
    };
    use base64::Engine as _;
    use std::sync::{
        Mutex,
        atomic::{AtomicU16, Ordering},
    };
    use zuse_cloud_projection::{TranscriptKey, reader::Reader};
    #[derive(Clone)]
    struct Server {
        status: Arc<AtomicU16>,
        paths: Arc<Mutex<Vec<String>>>,
    }
    async fn response(
        State(server): State<Server>,
        OriginalUri(uri): OriginalUri,
    ) -> impl IntoResponse {
        let path = uri.path();
        server.paths.lock().unwrap().push(path.into());
        assert!(path.starts_with("/v1/cloud/workspaces/workspace"));
        if path == "/v1/cloud/workspaces/workspace" {
            let status = StatusCode::from_u16(server.status.load(Ordering::SeqCst)).unwrap();
            return (
                status,
                axum::Json(
                    json!({"workspace":{"workspaceId":"workspace","chatId":"session","initialSessionId":"session","branch":"feature/work","state":"paused","desiredState":"paused","statusCode":"paused","revision":1,"rpcProtocol":"zuse-rpc-v1"}}),
                ),
            );
        }
        (
            StatusCode::NOT_FOUND,
            axum::Json(
                json!({"error": if path.ends_with("tool-output") {"tool_output_not_published"}else{"projection_not_found"}}),
            ),
        )
    }
    let server = Server {
        status: Arc::new(AtomicU16::new(503)),
        paths: Arc::new(Mutex::new(vec![])),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(response).with_state(server.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let api = CloudApi::new(&origin).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let account = crate::account::Account::fixture_signed_in(
        directory.path(),
        crate::account::API_URL.into(),
        "general-account",
    );
    let auth = Auth::new(crate::auth::AuthConfig::new(&origin, directory.path()))
        .with_account(Some(account));
    let client = Arc::new(zuse_rpc::memory_client(Arc::new(Controller { api, auth })));
    let root = std::path::PathBuf::from(std::env::var_os("ZUSE_CLOUD_HISTORY_CACHE_ROOT").unwrap());
    let reader = Reader::new(&origin)
        .unwrap()
        .with_scope("personal")
        .unwrap()
        .with_page_cache(&root, "general-account")
        .unwrap();
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([42; 32]);
    let key = TranscriptKey::parse(&encoded).unwrap();
    let head: Head = serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":zuse_cloud_projection::FORMAT,"workspaceId":"workspace","sessionId":"session","runtimeGeneration":2,"cursor":{"epoch":"epoch","version":1},"chat":{"id":"session","deviceId":"device","title":"Private history","archived":true,"cwd":"/repo","branch":"feature","checkoutId":null,"config":null,"lastMessagePreview":null,"lastMessageAt":null,"createdAt":"2026-10-23T00:00:00Z"},"conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":true,"olderMessageSequence":null})).unwrap();
    let upload = key
        .seal("workspace", "session", &head.cursor, None, &head)
        .unwrap();
    let projection = json!({"workspaceId":"workspace","sessionId":"session","cursor":head.cursor,"beforeSequence":null,"ciphertext":upload.ciphertext,"transcriptKey":encoded});
    let mut other = head.clone();
    other.workspace_id = "other".into();
    let sealed = key
        .seal("other", "session", &other.cursor, None, &other)
        .unwrap();
    let other_projection = json!({"workspaceId":"other","sessionId":"session","cursor":other.cursor,"beforeSequence":null,"ciphertext":sealed.ciphertext,"transcriptKey":encoded});
    let saved = |workspace: &str| {
        reader
            .cached_head("cache-token", workspace, "session")
            .unwrap()
            .is_some()
    };
    let seed = || {
        reader
            .cache_projection("cache-token", &other_projection)
            .unwrap();
        reader.cache_projection("cache-token", &projection).unwrap();
        assert!(
            reader
                .cached_head("cache-token", "workspace", "session")
                .unwrap()
                .is_some()
        );
    };
    // What a restarted app paints first: the platform-protected saved head,
    // read without a token or any in-memory lease.
    let provisional = |workspace: &'static str| {
        let client = client.clone();
        async move {
            client
                .call_scoped(READ, json!({"workspaceId":workspace,"sessionId":"session","cacheOnly":true,"expectedAccountId":"general-account"}))
                .await
                .unwrap()["head"]
                .is_object()
        }
    };
    let params = json!({"workspaceId":"workspace","expectedAccountId":"general-account"});
    for status in [503, 404, 401, 403] {
        seed();
        server.status.store(status, Ordering::SeqCst);
        let error = client.call_scoped(READ, params.clone()).await.unwrap_err();
        if status == 404 {
            assert!(
                matches!(error, zuse_rpc::RpcError::Capability(ref code) if code == "workspace_not_found"),
                "{error}"
            );
        } else {
            assert!(!error.to_string().contains("workspace_not_found"));
        }
        // 404 proves deletion and 403 withdraws access: forget that workspace.
        // A 401 is an expired bearer, and a 503 says nothing about access.
        assert_eq!(
            saved("workspace"),
            matches!(status, 503 | 401),
            "metadata status {status}"
        );
        assert!(
            saved("other"),
            "metadata status {status} for one workspace keeps other chats"
        );
        assert_eq!(
            provisional("workspace").await,
            matches!(status, 503 | 401),
            "restart paint after metadata status {status}"
        );
        assert!(provisional("other").await, "status {status}");
    }
    server.status.store(200, Ordering::SeqCst);
    seed();
    let error = client.call_scoped(READ, json!({"workspaceId":"workspace","expectedAccountId":"general-account","toolBlobRef":"blob","offset":0})).await.unwrap_err();
    assert!(error.to_string().contains("not finished syncing"));
    assert!(
        saved("workspace") && provisional("workspace").await,
        "unpublished tool404 must preserve encrypted history"
    );
    let _ = client.call_scoped(READ, params).await;
    assert!(
        saved("workspace") && provisional("workspace").await,
        "task-history/projection404 must preserve encrypted history"
    );
    assert!(
        server
            .paths
            .lock()
            .unwrap()
            .iter()
            .all(|path| !path.contains("wake") && !path.contains("commands"))
    );
    task.abort();
}
