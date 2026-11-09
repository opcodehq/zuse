use super::*;
// Append inside cloud/hosted.rs only after the backend source freeze is released.
#[cfg(test)]
mod hosted_controller_tests {
    use super::*;
    use axum::{
        Router,
        extract::{Request, State},
        response::IntoResponse,
    };
    use std::sync::Mutex;

    type Observed = Arc<Mutex<Vec<(String, String, Option<String>, String)>>>;
    async fn backend(
        State(observed): State<Observed>,
        request: Request,
    ) -> axum::response::Response {
        let method = request.method().as_str().to_owned();
        let path = request.uri().path().to_owned();
        let token = request
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        let user = token
            .split('.')
            .nth(1)
            .and_then(|payload| {
                use base64::Engine;
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(payload)
                    .ok()
            })
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|v| v["sub"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let org = request
            .headers()
            .get("x-zuse-workspace")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("organization:"))
            .map(str::to_owned);
        observed
            .lock()
            .unwrap()
            .push((method, path.clone(), org.clone(), user.clone()));
        if org.as_deref() == Some("org-a") && user != "actor-a" {
            return axum::http::StatusCode::FORBIDDEN.into_response();
        }
        if path == "/v1/cloud/workspaces/workspace/task-commands" {
            let bytes = axum::body::to_bytes(request.into_body(), 128 * 1024)
                .await
                .unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            return axum::Json(json!({"durablyAdmitted":true,"commandId":request["commandId"]}))
                .into_response();
        }
        if path == "/v1/cloud/workspaces/workspace" {
            return axum::Json(json!({"workspace":{"workspaceId":"workspace","chatId":"session","initialSessionId":"session","branch":"retained-branch","state":"paused","desiredState":"paused","statusCode":"paused","revision":1,"rpcProtocol":"zuse-rpc-v1"}})).into_response();
        }
        if path == "/v1/cloud/workspaces/workspace/sessions/session/task-history" {
            return axum::Json(json!({"source":"runtime","head":{"schemaVersion":1,"payloadFormat":"zuse-cloud-admission-v1","workspaceId":"workspace","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"epoch","version":1},"chat":{"id":"session","deviceId":"runtime","archived":false,"createdAt":"2026-01-01T00:00:00Z","threadId":"thread"},"conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false}})).into_response();
        }
        axum::Json(json!({"accountId":user,"projects":[],"workspaces":[],"organizationId":org}))
            .into_response()
    }
    fn controller(origin: &str, actor: &str) -> Arc<Controller> {
        let account = Account::ephemeral_fixture(origin, origin, "public-staging-client").unwrap();
        account.fixture_install(actor);
        let (scope, receiver) = watch::channel((None, None));
        let auth = CloudIdentity::hosted(account.clone(), receiver);
        Arc::new(Controller {
            account,
            api: Arc::new(CloudApi::hosted(origin).unwrap()),
            auth,
            scope,
            controller_id: format!("controller-{actor}"),
            organizations: Arc::new(vec!["org-a".into()]),
            requests: Arc::new(Semaphore::new(32)),
            login_slots: Arc::new(Semaphore::new(4)),
            shared_requests: Arc::new(Semaphore::new(32)),
            shared_controls: Arc::new(Semaphore::new(4)),
            control_requests: Arc::new(Semaphore::new(2)),
            last_login: Mutex::new(None),
            closed: CancellationToken::new(),
        })
    }
    #[tokio::test]
    async fn simultaneous_cloud_actors_use_explicit_api_and_scopes_without_local_fallback() {
        let observed: Observed = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().fallback(backend).with_state(observed.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let a = controller(&origin, "actor-a");
        let b = controller(&origin, "actor-b");
        let (ra, rb) = tokio::join!(
            a.handle(
                methods::CLOUD_CONTEXT,
                json!({"expectedAccountId":"actor-a"})
            ),
            b.handle(
                methods::CLOUD_CONTEXT,
                json!({"expectedAccountId":"actor-b"})
            )
        );
        assert!(ra.is_ok() && rb.is_ok());
        a.handle(methods::SELECT_ORG, json!({"organizationId":"org-a"}))
            .await
            .unwrap();
        assert!(
            b.handle(methods::SELECT_ORG, json!({"organizationId":"org-a"}))
                .await
                .is_err()
        );
        assert_eq!(a.current(), (Some("actor-a".into()), Some("org-a".into())));
        assert_eq!(b.current(), (Some("actor-b".into()), None));
        let RpcReply::Value(history) = a.handle(methods::CLOUD_HISTORY,json!({"workspaceId":"workspace","sessionId":"session","incremental":true,"expectedAccountId":"actor-a","expectedOrganizationId":"org-a"})).await.unwrap() else {panic!("expected saved history")};
        assert_eq!(history["head"]["sessionId"], "session");
        assert_eq!(history["workspace"]["state"], "paused");
        let before = observed.lock().unwrap().len();
        assert!(
            b.handle(
                methods::CLOUD_CONTEXT,
                json!({"expectedAccountId":"actor-a"})
            )
            .await
            .is_err()
        );
        for method in [
            "ReadFile",
            "CreateTerminal",
            "RunCommand",
            "GetWorkspaceState",
        ] {
            assert!(a.handle(method, json!({})).await.is_err());
        }
        assert_eq!(observed.lock().unwrap().len(), before);
        assert!(observed.lock().unwrap().iter().all(|row| row.0 == "GET"
            && (row.1 == "/v1/cloud/context"
                || row.1.starts_with("/v1/cloud/workspaces/workspace"))));
        assert!(
            observed
                .lock()
                .unwrap()
                .iter()
                .any(|row| row.3 == "actor-a")
        );
        assert!(
            observed
                .lock()
                .unwrap()
                .iter()
                .any(|row| row.3 == "actor-b")
        );
        task.abort();
        let _ = task.await;
    }
    #[tokio::test]
    async fn session_subscription_budget_refunds_and_scope_change_cancels_old_stream() {
        let controller = controller("http://127.0.0.1:1", "actor-a");
        let mut replies = Vec::new();
        for _ in 0..32 {
            replies.push(
                controller
                    .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                    .await
                    .unwrap(),
            );
        }
        assert!(
            controller
                .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
                .is_err()
        );
        replies.pop();
        assert!(
            controller
                .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
                .is_ok()
        );
        drop(replies);
        assert_eq!(controller.requests.available_permits(), 32);
        let permit = (
            controller.requests.clone().try_acquire_owned().unwrap(),
            controller
                .shared_requests
                .clone()
                .try_acquire_owned()
                .unwrap(),
        );
        let reply = controller.bounded(
            RpcReply::Stream(futures::stream::pending().boxed()),
            permit,
            Some(controller.current()),
        );
        controller
            .scope
            .send_replace((Some("actor-a".into()), Some("org-a".into())));
        let RpcReply::Stream(mut stream) = reply else {
            panic!("expected stream")
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await
                .unwrap()
                .is_none()
        );
        drop(stream);
        assert_eq!(controller.requests.available_permits(), 32);
    }
    #[tokio::test]
    async fn hosted_sign_in_never_starts_device_authorization() {
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = requests.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(move || {
                    seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { axum::http::StatusCode::NOT_FOUND }
                }),
            )
            .await
            .unwrap();
        });
        let controller = controller(&origin, "actor-a");
        controller.account.sign_out().unwrap();
        let error = controller
            .handle(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}))
            .await
            .err()
            .expect("hosted sign-in has no device fallback");
        assert!(error.to_string().contains("Sign in from Zuse"));
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0);
        controller.account.retire();
        controller.closed.cancel();
        task.abort();
        let _ = task.await;
    }
    #[tokio::test]
    async fn factory_sessions_share_the_global_request_budget() {
        use zuse_serve::hosted::Factory as _;
        assert!(Factory::new("http://127.0.0.1:1", "public-staging-client", vec![]).is_err());
        let factory =
            Factory::new("https://cloud.example", "public-staging-client", vec![]).unwrap();
        let a = factory.create("controller-a").unwrap();
        let b = factory.create("controller-b").unwrap();
        let mut streams = Vec::new();
        for _ in 0..32 {
            streams.push(
                a.rpc
                    .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                    .await
                    .unwrap(),
            );
        }
        assert!(
            b.rpc
                .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
                .is_err()
        );
        streams.pop();
        assert!(
            b.rpc
                .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
                .is_ok()
        );
        drop(streams);
        assert_eq!(factory.shared_requests.available_permits(), 32);
        a.closed.cancel();
        b.closed.cancel();
        (a.retire)();
        (b.retire)();
    }
    #[test]
    fn hosted_denial_and_logout_do_not_read_or_purge_native_encrypted_cache() {
        const CHILD: &str = "ZUSE_HOSTED_NATIVE_CACHE_ISOLATION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let directory = tempfile::tempdir().unwrap();
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","cloud::hosted::hosted_tests::hosted_controller_tests::hosted_denial_and_logout_do_not_read_or_purge_native_encrypted_cache","--nocapture"])
                .env(CHILD,"1").env("ZUSE_CLOUD_HISTORY_CACHE_ROOT",directory.path()).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            use zuse_cloud_projection::{TranscriptKey,reader::Reader};
            use base64::Engine;
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin=format!("http://{}",listener.local_addr().unwrap());
            let task=tokio::spawn(async move {axum::serve(listener,Router::new().fallback(||async{axum::http::StatusCode::UNAUTHORIZED})).await.unwrap();});
            let root=std::path::PathBuf::from(std::env::var_os("ZUSE_CLOUD_HISTORY_CACHE_ROOT").unwrap());
            let reader=Reader::new(&origin).unwrap().with_scope("personal").unwrap().with_page_cache(&root,"actor-a").unwrap();
            let encoded=base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([42;32]);
            let key=TranscriptKey::parse(&encoded).unwrap();
            let head:zuse_model::cloud::Head=serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":zuse_cloud_projection::FORMAT,"workspaceId":"workspace","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"epoch","version":1},"chat":{"id":"session","deviceId":"device","archived":true,"createdAt":"2026-01-01T00:00:00Z"},"conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false})).unwrap();
            let sealed=key.seal("workspace","session",&head.cursor,None,&head).unwrap();
            reader.cache_projection("native-key",&json!({"workspaceId":"workspace","sessionId":"session","cursor":head.cursor,"beforeSequence":null,"ciphertext":sealed.ciphertext,"transcriptKey":encoded})).unwrap();
            assert!(reader.cached_head("native-key","workspace","session").unwrap().is_some());
            let controller=controller(&origin,"actor-a");
            let RpcReply::Value(provisional)=controller.handle(methods::CLOUD_HISTORY,json!({"workspaceId":"workspace","sessionId":"session","expectedAccountId":"actor-a","cacheOnly":true})).await.unwrap()else{panic!("expected hosted cache-only reply")};
            assert!(provisional["head"].is_null());
            assert!(controller.handle(methods::CLOUD_HISTORY,json!({"workspaceId":"workspace","sessionId":"session","expectedAccountId":"actor-a"})).await.is_err());
            controller.handle(methods::CLOUD_ACCOUNT_SIGN_OUT,json!({})).await.unwrap();
            assert!(reader.cached_head("native-key","workspace","session").unwrap().is_some());
            task.abort();let _=task.await;
        });
    }
    #[tokio::test]
    async fn saturated_history_lane_keeps_stop_queue_signout_and_cancellation_available() {
        let observed: Observed = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(backend).with_state(observed),
            )
            .await
            .unwrap();
        });
        let controller = controller(&origin, "actor-a");
        let mut streams = Vec::new();
        for _ in 0..32 {
            streams.push(
                controller
                    .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                    .await
                    .unwrap(),
            );
        }
        assert!(
            controller
                .handle(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
                .is_err()
        );
        assert!(
            controller
                .handle(methods::AUTH_STATUS, json!({}))
                .await
                .is_ok()
        );
        for (id, action) in [
            (
                "stop",
                json!({"type":"submit","messageId":"stop-message","payload":{"kind":"interrupt"}}),
            ),
            ("queue-pause", json!({"type":"pause-queue","paused":true})),
        ] {
            controller.handle(methods::CLOUD_TASK_COMMAND,json!({"workspaceId":"workspace","expectedAccountId":"actor-a","expectedOrganizationId":null,"request":{"commandId":id,"sessionId":"session","action":action}})).await.unwrap();
        }
        let one = controller
            .control_requests
            .clone()
            .try_acquire_owned()
            .unwrap();
        let two = controller
            .control_requests
            .clone()
            .try_acquire_owned()
            .unwrap();
        assert!(
            controller
                .handle(methods::AUTH_STATUS, json!({}))
                .await
                .is_err()
        );
        drop(one);
        drop(two);
        controller
            .handle(methods::CLOUD_ACCOUNT_SIGN_OUT, json!({}))
            .await
            .unwrap();
        assert!(controller.closed.is_cancelled());
        assert!(controller.account.status().user.is_none());
        drop(streams);
        assert_eq!(controller.requests.available_permits(), 32);
        assert_eq!(controller.shared_requests.available_permits(), 32);
        task.abort();
        let _ = task.await;
    }
}
