//! Auth service tests: dev mode, and the WorkOS flows (headless paste-code exchange,
//! loopback callback, refresh rotation + revocation, org onboarding) against a stub
//! edge HTTP server on a plain tokio TcpListener.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use zuse_engine::{Auth, AuthConfig, AuthState};
use zuse_rpc::{TokenError, TokenSource};

// ---------------------------------------------------------------------------
// Fake JWTs
// ---------------------------------------------------------------------------

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        }
    }
    out
}

/// An unsigned JWT with the claims the engine reads (`exp`/`iat` for TTL, `org_id`).
fn fake_jwt(ttl_secs: i64, org_id: Option<&str>) -> String {
    let mut claims = serde_json::json!({ "sub": "user_1", "iat": 1_000, "exp": 1_000 + ttl_secs });
    if let Some(org) = org_id {
        claims["org_id"] = serde_json::json!(org);
    }
    format!("e30.{}.sig", base64url(claims.to_string().as_bytes()))
}

// ---------------------------------------------------------------------------
// Stub edge server
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StubState {
    exchanges: AtomicUsize,
    block_exchange: AtomicBool,
    exchange_started: tokio::sync::Notify,
    release_exchange: tokio::sync::Notify,
    refreshes: AtomicUsize,
    drop_refresh: AtomicBool,
    refresh_status: AtomicUsize,
    block_refresh: AtomicBool,
    refresh_started: tokio::sync::Notify,
    release_refresh: tokio::sync::Notify,
    /// Refresh tokens seen by /auth/refresh, in order.
    refresh_tokens: Mutex<Vec<String>>,
    /// TTL (seconds) for minted access tokens.
    token_ttl: AtomicUsize,
    /// org_id claim for exchange-minted tokens ("" = none).
    exchange_org: Mutex<String>,
}

struct StubEdge {
    port: u16,
    state: Arc<StubState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for StubEdge {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl StubEdge {
    async fn start() -> StubEdge {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
        let port = listener.local_addr().expect("addr").port();
        let state = Arc::new(StubState::default());
        state.token_ttl.store(3600, Ordering::SeqCst);
        let handler_state = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(handle(stream, handler_state.clone()));
            }
        });
        StubEdge { port, state, task }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next()?.to_string();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    Some((method, target, String::from_utf8_lossy(&body).into_owned()))
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn handle(mut stream: tokio::net::TcpStream, state: Arc<StubState>) {
    let Some((method, target, body)) = read_request(&mut stream).await else {
        return;
    };
    let path = target.split('?').next().unwrap_or("");
    let ttl = state.token_ttl.load(Ordering::SeqCst) as i64;
    match (method.as_str(), path) {
        ("GET", "/health") => {
            respond(&mut stream, "200 OK", r#"{"ok":true,"auth":"workos"}"#).await;
        }
        ("POST", "/auth/exchange") => {
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if parsed.get("code").and_then(|v| v.as_str()).is_none() {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    r#"{"error":"missing code"}"#,
                )
                .await;
                return;
            }
            let n = state.exchanges.fetch_add(1, Ordering::SeqCst) + 1;
            if state.block_exchange.load(Ordering::SeqCst) {
                state.exchange_started.notify_one();
                state.release_exchange.notified().await;
            }
            let org = state.exchange_org.lock().expect("lock").clone();
            let token = fake_jwt(ttl, (!org.is_empty()).then_some(org.as_str()));
            let response = serde_json::json!({
                "user": { "id": "user_1", "email": "w@example.com",
                          "firstName": "Wing", "lastName": "Test" },
                "accessToken": token,
                "refreshToken": format!("refresh-{n}"),
            });
            respond(&mut stream, "200 OK", &response.to_string()).await;
        }
        ("POST", "/auth/refresh") => {
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let refresh_token = parsed
                .get("refreshToken")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            state
                .refresh_tokens
                .lock()
                .expect("lock")
                .push(refresh_token.to_string());
            if state.block_refresh.load(Ordering::SeqCst) {
                state.refresh_started.notify_one();
                state.release_refresh.notified().await;
            }
            if state.drop_refresh.load(Ordering::SeqCst) {
                state.refreshes.fetch_add(1, Ordering::SeqCst);
                return;
            }
            let status = state.refresh_status.load(Ordering::SeqCst);
            if status != 0 {
                state.refreshes.fetch_add(1, Ordering::SeqCst);
                respond(&mut stream, &format!("{status} Unavailable"), "{}").await;
                return;
            }
            if refresh_token == "dead" {
                respond(&mut stream, "401 Unauthorized", r#"{"error":"revoked"}"#).await;
                return;
            }
            let n = state.refreshes.fetch_add(1, Ordering::SeqCst) + 1;
            let org = parsed
                .get("organizationId")
                .and_then(|v| v.as_str())
                .or(Some("org_1"));
            let response = serde_json::json!({
                "accessToken": fake_jwt(ttl, org),
                "refreshToken": format!("rotated-{n}"),
            });
            respond(&mut stream, "200 OK", &response.to_string()).await;
        }
        ("GET", "/auth/orgs") => {
            respond(
                &mut stream,
                "200 OK",
                r#"{"orgs":[{"id":"om_1","organizationId":"org_1","name":"Acme"}]}"#,
            )
            .await;
        }
        ("POST", "/auth/orgs") => {
            respond(&mut stream, "200 OK", r#"{"organizationId":"org_new"}"#).await;
        }
        _ => respond(&mut stream, "404 Not Found", r#"{"error":"not_found"}"#).await,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn workos_config(edge_url: &str, data_dir: &std::path::Path) -> AuthConfig {
    let mut config = AuthConfig::new(edge_url, data_dir);
    config.workos_client_id = Some("client_test".into());
    config.workos_api_base = "https://authkit.example".into();
    config
}

fn query_param(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?
        .1
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
}

async fn wait_for<T>(rx: &mut tokio::sync::watch::Receiver<T>, check: impl Fn(&T) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if check(&rx.borrow()) {
                return;
            }
            rx.changed().await.expect("state channel open");
        }
    })
    .await
    .expect("state reached in time");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dev_mode_is_signed_in_with_configured_bearer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = AuthConfig::new("http://127.0.0.1:1", dir.path());
    config.dev_user_id = "wing-dev".into();
    let auth = Auth::new(config);
    assert!(!auth.workos_enabled());
    assert!(!auth.loaded_workos_session());
    assert!(matches!(auth.state(), AuthState::SignedIn { user, .. } if user.id == "wing-dev"));
    assert_eq!(auth.access_token().await.as_deref(), Ok("wing-dev"));
    // Dev sign-in mirrors the TS service: a no-op URL, CompleteSignIn accepted.
    assert_eq!(auth.start_sign_in().await.expect("dev sign-in"), "");
    auth.complete_sign_in("whatever")
        .await
        .expect("dev complete is a no-op");
}

#[tokio::test]
async fn headless_flow_exchanges_pasted_code_and_gates_on_org() {
    let edge = StubEdge::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let auth = Auth::new(workos_config(&edge.url(), dir.path()));
    assert!(auth.workos_enabled());
    assert!(!auth.loaded_workos_session());
    assert_eq!(auth.state(), AuthState::SignedOut);
    assert_eq!(
        auth.access_token().await,
        Err(TokenError::SignedOut),
        "signed out: no token"
    );

    let url = auth.start_headless_sign_in();
    assert!(url.starts_with("https://authkit.example/user_management/authorize?"));
    assert_eq!(
        query_param(&url, "client_id").as_deref(),
        Some("client_test")
    );
    let redirect = query_param(&url, "redirect_uri").expect("redirect");
    assert!(
        redirect.contains("auth%2Fcli%2Fcallback"),
        "hosted paste-code page: {redirect}"
    );
    let state = query_param(&url, "state").expect("state param");

    // A code minted for someone else's flow (unknown state) is rejected — CSRF check.
    assert!(auth.complete_sign_in("bogus-state.code123").await.is_err());

    // The real paste: `state.code`. The exchange-minted token carries no org claim, so
    // the session lands in NeedsOrganization (the org gate).
    auth.complete_sign_in(&format!("{state}.code123"))
        .await
        .expect("paste-code sign-in");
    assert!(
        !auth.loaded_workos_session(),
        "sign-in does not rewrite the startup fact"
    );
    assert_eq!(edge.state.exchanges.load(Ordering::SeqCst), 1);
    assert!(
        matches!(auth.state(), AuthState::NeedsOrganization { user } if user.email == "w@example.com")
    );

    // Session persisted 0600 with the exchange's refresh token.
    let session_file = dir.path().join("session.json");
    let raw = std::fs::read_to_string(&session_file).expect("session persisted");
    assert!(raw.contains("refresh-1"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&session_file)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "session file must be private");
    }

    // Org onboarding: list, then select — an org-scoped refresh; state follows the
    // returned token's org claim.
    let orgs = auth.list_orgs().await.expect("list orgs");
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0].organization_id, "org_1");
    let mut token_changes = auth.subscribe().expect("auth token signal");
    auth.select_org("org_1").await.expect("select org");
    token_changes
        .changed()
        .await
        .expect("org-scoped token should wake transport supervisors");
    assert!(
        matches!(auth.state(), AuthState::SignedIn { org_id: Some(org), .. } if org == "org_1")
    );
    assert!(auth.token().await.is_ok());
    assert_eq!(
        edge.state
            .refresh_tokens
            .lock()
            .expect("lock")
            .first()
            .map(String::as_str),
        Some("refresh-1"),
        "org refresh presents the stored refresh token"
    );
    // Rotation persisted.
    let raw = std::fs::read_to_string(&session_file).expect("session persisted");
    assert!(
        raw.contains("rotated-1"),
        "rotated refresh token stored: {raw}"
    );

    // Sign-out clears state and removes the persisted session.
    auth.sign_out();
    assert_eq!(auth.state(), AuthState::SignedOut);
    assert!(!session_file.exists());
}

#[tokio::test]
async fn short_lived_tokens_refresh_on_demand() {
    let edge = StubEdge::start().await;
    // Tokens live 20s < the 30s slack → every access_token() call refreshes.
    edge.state.token_ttl.store(20, Ordering::SeqCst);
    *edge.state.exchange_org.lock().expect("lock") = "org_1".into();
    let dir = tempfile::tempdir().expect("tempdir");
    let auth = Auth::new(workos_config(&edge.url(), dir.path()));

    let url = auth.start_headless_sign_in();
    let state = query_param(&url, "state").expect("state");
    auth.complete_sign_in(&format!("{state}.codeX"))
        .await
        .expect("sign in");
    assert!(auth.state().is_signed_in());

    let first = auth.access_token().await.expect("token after refresh");
    assert_eq!(
        edge.state.refreshes.load(Ordering::SeqCst),
        1,
        "stale exchange token refreshed"
    );
    let second = auth.access_token().await.expect("token again");
    assert_eq!(
        edge.state.refreshes.load(Ordering::SeqCst),
        2,
        "still under slack → refreshed"
    );
    assert_eq!(first, second, "same claims → same fake token bytes");
    // Rotated refresh tokens are chained: refresh N presents rotation N-1's token.
    let seen = edge.state.refresh_tokens.lock().expect("lock").clone();
    assert_eq!(seen, vec!["refresh-1".to_string(), "rotated-1".to_string()]);
}

#[tokio::test]
async fn revoked_refresh_token_signs_out() {
    let edge = StubEdge::start().await;
    let dir = tempfile::tempdir().expect("tempdir");
    // A persisted session whose refresh token the edge rejects with a definitive 4xx.
    std::fs::write(
        dir.path().join("session.json"),
        r#"{"refreshToken":"dead","user":{"id":"user_1","email":"w@example.com"},"orgId":"org_1"}"#,
    )
    .expect("seed session");
    let auth = Auth::new(workos_config(&edge.url(), dir.path()));
    assert!(
        auth.state().is_signed_in(),
        "boots from the persisted session"
    );
    assert!(auth.loaded_workos_session());

    // The refresh is doomed → the session degrades to SignedOut and the file is gone.
    assert_eq!(auth.access_token().await, Err(TokenError::SignedOut));
    assert_eq!(auth.state(), AuthState::SignedOut);
    assert!(
        auth.loaded_workos_session(),
        "revocation must not rewrite the captured startup fact"
    );
    assert!(!dir.path().join("session.json").exists());
}

#[tokio::test]
async fn offline_refresh_loop_backs_off_without_revoking_session() {
    let edge = StubEdge::start().await;
    edge.state.drop_refresh.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().expect("tempdir");
    let session_file = dir.path().join("session.json");
    std::fs::write(
        &session_file,
        r#"{"refreshToken":"offline","user":{"id":"user_1","email":"w@example.com"},"orgId":"org_1"}"#,
    )
    .expect("seed session");
    let auth = Auth::new(workos_config(&edge.url(), dir.path()));

    let refresh_loop = auth.spawn_refresh_loop();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        edge.state.refreshes.load(Ordering::SeqCst),
        1,
        "a transport failure must enter the background retry delay"
    );
    assert!(auth.state().is_signed_in());
    assert!(session_file.exists(), "offline must not revoke the session");
    refresh_loop.abort();
}

fn persisted_auth(edge: &StubEdge, dir: &std::path::Path) -> Auth {
    std::fs::write(
        dir.join("session.json"),
        r#"{"refreshToken":"offline","user":{"id":"user_1","email":"w@example.com"},"orgId":"org_1"}"#,
    ).expect("seed session");
    Auth::new(workos_config(&edge.url(), dir))
}

async fn twenty_consumers(auth: &Auth) -> Vec<Result<String, TokenError>> {
    let barrier = Arc::new(tokio::sync::Barrier::new(20));
    let mut consumers = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let auth = auth.clone();
        let barrier = barrier.clone();
        consumers.spawn(async move {
            barrier.wait().await;
            auth.access_token().await
        });
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut results = Vec::new();
        while let Some(result) = consumers.join_next().await {
            results.push(result.expect("consumer task"));
        }
        results
    })
    .await
    .expect("consumers finish")
}

#[tokio::test]
async fn twenty_consumers_and_background_loop_share_a_failed_refresh() {
    let edge = StubEdge::start().await;
    edge.state.drop_refresh.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let auth = persisted_auth(&edge, dir.path());
    let refresh_loop = auth.spawn_refresh_loop();

    assert!(
        twenty_consumers(&auth)
            .await
            .iter()
            .all(|result| matches!(result, Err(TokenError::TemporarilyUnavailable(_))))
    );
    assert_eq!(edge.state.refreshes.load(Ordering::SeqCst), 1);
    assert!(
        twenty_consumers(&auth)
            .await
            .iter()
            .all(|result| matches!(result, Err(TokenError::TemporarilyUnavailable(_))))
    );
    assert_eq!(
        edge.state.refreshes.load(Ordering::SeqCst),
        1,
        "new consumers must also respect the shared cooldown"
    );
    assert!(auth.state().is_signed_in());
    assert!(dir.path().join("session.json").exists());
    refresh_loop.abort();
}

#[tokio::test]
async fn temporary_token_errors_reach_http_and_websocket_consumers() {
    use zuse_sync::chat_client::ChatTransport;
    let edge = StubEdge::start().await;
    edge.state.drop_refresh.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let auth = persisted_auth(&edge, dir.path());
    let edge_config = zuse_engine::EdgeConfig::new(edge.url(), Arc::new(auth.clone()));

    let websocket = edge_config.room_url("/registry/org_1/ws").url().await;
    assert!(matches!(
        websocket,
        Err(zuse_sync::SyncError::TemporarilyUnavailable(_))
    ));
    let transport = zuse_engine::chat2_host::EdgeChatTransport::new(
        reqwest::Client::new(),
        edge_config,
        "chat",
        "device",
    );
    assert!(matches!(
        transport.fetch_rows(0).await,
        Err(zuse_sync::SyncError::TemporarilyUnavailable(_))
    ));
    assert!(matches!(
        auth.list_orgs().await,
        Err(zuse_engine::EngineError::Token(
            TokenError::TemporarilyUnavailable(_)
        ))
    ));
    assert_eq!(edge.state.refreshes.load(Ordering::SeqCst), 1);
    assert!(auth.state().is_signed_in());
}

#[tokio::test]
async fn transient_http_statuses_do_not_revoke_the_session() {
    for status in [408, 429, 503] {
        let edge = StubEdge::start().await;
        edge.state.refresh_status.store(status, Ordering::SeqCst);
        let dir = tempfile::tempdir().unwrap();
        let auth = persisted_auth(&edge, dir.path());
        assert!(
            matches!(
                auth.access_token().await,
                Err(TokenError::TemporarilyUnavailable(_))
            ),
            "HTTP {status} must remain retryable"
        );
        assert!(auth.state().is_signed_in());
        assert!(dir.path().join("session.json").exists());
    }
}

#[tokio::test]
async fn refresh_recovers_after_cooldown_without_a_network_event() {
    let edge = StubEdge::start().await;
    edge.state.drop_refresh.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let auth = persisted_auth(&edge, dir.path());
    assert!(auth.access_token().await.is_err());
    edge.state.drop_refresh.store(false, Ordering::SeqCst);
    let mut tokens = auth.subscribe().unwrap();
    let refresh_loop = auth.spawn_refresh_loop();

    tokio::time::timeout(Duration::from_secs(3), tokens.changed())
        .await
        .expect("automatic recovery must remain prompt")
        .unwrap();
    assert!(auth.access_token().await.is_ok());
    assert!(auth.state().is_signed_in());
    assert_eq!(edge.state.refreshes.load(Ordering::SeqCst), 2);
    refresh_loop.abort();
}

#[tokio::test]
async fn explicit_retry_allows_one_shared_attempt_before_cooldown_expires() {
    let edge = StubEdge::start().await;
    edge.state.drop_refresh.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let auth = persisted_auth(&edge, dir.path());
    assert!(auth.access_token().await.is_err());
    edge.state.drop_refresh.store(false, Ordering::SeqCst);

    auth.retry_refresh();
    assert!(twenty_consumers(&auth).await.iter().all(Result::is_ok));
    assert_eq!(edge.state.refreshes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancelling_a_consumer_does_not_cancel_the_shared_refresh() {
    let edge = StubEdge::start().await;
    edge.state.block_refresh.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let auth = persisted_auth(&edge, dir.path());
    let first = tokio::spawn({
        let auth = auth.clone();
        async move { auth.access_token().await }
    });
    tokio::time::timeout(
        Duration::from_secs(3),
        edge.state.refresh_started.notified(),
    )
    .await
    .unwrap();
    first.abort();
    edge.state.block_refresh.store(false, Ordering::SeqCst);
    edge.state.release_refresh.notify_one();

    assert!(auth.access_token().await.is_ok());
    assert_eq!(edge.state.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        edge.state.refresh_tokens.lock().unwrap().as_slice(),
        ["offline"]
    );
}

#[tokio::test]
async fn sign_out_during_shared_refresh_does_not_restore_credentials() {
    let edge = StubEdge::start().await;
    edge.state.block_refresh.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let auth = persisted_auth(&edge, dir.path());
    let pending = tokio::spawn({
        let auth = auth.clone();
        async move { auth.access_token().await }
    });
    tokio::time::timeout(
        Duration::from_secs(3),
        edge.state.refresh_started.notified(),
    )
    .await
    .unwrap();
    auth.sign_out();
    edge.state.release_refresh.notify_one();

    assert!(pending.await.unwrap() == Err(TokenError::SignedOut));
    assert!(auth.access_token().await.is_err());
    assert_eq!(auth.state(), AuthState::SignedOut);
    assert!(!dir.path().join("session.json").exists());
}

#[tokio::test]
async fn an_old_refresh_cannot_report_signed_out_after_a_new_sign_in() {
    for status in [200, 401, 503] {
        let edge = StubEdge::start().await;
        edge.state.block_refresh.store(true, Ordering::SeqCst);
        if status != 200 {
            edge.state.refresh_status.store(status, Ordering::SeqCst);
        }
        *edge.state.exchange_org.lock().unwrap() = "org_new".into();
        let dir = tempfile::tempdir().unwrap();
        let auth = persisted_auth(&edge, dir.path());
        let pending = tokio::spawn({
            let auth = auth.clone();
            async move { auth.access_token().await }
        });
        tokio::time::timeout(
            Duration::from_secs(3),
            edge.state.refresh_started.notified(),
        )
        .await
        .unwrap();

        auth.sign_out();
        let url = auth.start_headless_sign_in();
        let state = query_param(&url, "state").unwrap();
        auth.complete_sign_in(&format!("{state}.new-code"))
            .await
            .unwrap();
        let fresh = auth.access_token().await.unwrap();
        edge.state.release_refresh.notify_one();

        assert_eq!(
            pending.await.unwrap(),
            Ok(fresh),
            "old HTTP {status} response must not hide the new session"
        );
        assert!(
            matches!(auth.state(), AuthState::SignedIn { org_id: Some(org), .. } if org == "org_new")
        );
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("session.json")).unwrap())
                .unwrap();
        assert_eq!(stored["refreshToken"], "refresh-1");
    }
}

#[tokio::test]
async fn loopback_callback_completes_headed_sign_in() {
    let edge = StubEdge::start().await;
    *edge.state.exchange_org.lock().expect("lock") = "org_1".into();
    let dir = tempfile::tempdir().expect("tempdir");
    let auth = Auth::new(workos_config(&edge.url(), dir.path()));

    let url = auth.start_sign_in().await.expect("authorize url");
    let redirect = query_param(&url, "redirect_uri").expect("redirect");
    assert!(
        redirect.starts_with("http%3A%2F%2F127.0.0.1%3A"),
        "loopback redirect: {redirect}"
    );
    let state = query_param(&url, "state").expect("state");
    let callback: String = redirect.replace("%3A", ":").replace("%2F", "/");

    // A wrong/expired state is rejected without touching the exchange endpoint.
    let bad = reqwest::get(format!("{callback}?code=abc&state=wrong"))
        .await
        .expect("bad cb");
    assert_eq!(bad.status().as_u16(), 400);
    assert_eq!(edge.state.exchanges.load(Ordering::SeqCst), 0);

    // The browser hits the loopback callback → the engine exchanges the code with the
    // edge and the session lands org-scoped.
    let ok = reqwest::get(format!("{callback}?code=abc&state={state}"))
        .await
        .expect("cb");
    assert_eq!(ok.status().as_u16(), 200);
    let mut state_rx = auth.watch_state();
    wait_for(&mut state_rx, |s| s.is_signed_in()).await;
    assert_eq!(edge.state.exchanges.load(Ordering::SeqCst), 1);
    assert!(
        matches!(auth.state(), AuthState::SignedIn { org_id: Some(org), user } if org == "org_1" && user.name.as_deref() == Some("Wing Test"))
    );
}

#[tokio::test]
async fn sign_out_invalidates_pending_and_in_flight_oauth_callbacks() {
    let edge = StubEdge::start().await;
    *edge.state.exchange_org.lock().expect("lock") = "org_1".into();
    let dir = tempfile::tempdir().expect("tempdir");
    let auth = Auth::new(workos_config(&edge.url(), dir.path()));

    let first_url = auth.start_sign_in().await.expect("authorize url");
    let callback = query_param(&first_url, "redirect_uri")
        .expect("redirect")
        .replace("%3A", ":")
        .replace("%2F", "/");
    let first_state = query_param(&first_url, "state").expect("state");
    auth.sign_out();
    let canceled_pending = reqwest::get(format!("{callback}?code=old&state={first_state}"))
        .await
        .expect("pending callback response");
    assert_eq!(canceled_pending.status().as_u16(), 400);
    assert_eq!(edge.state.exchanges.load(Ordering::SeqCst), 0);

    edge.state.block_exchange.store(true, Ordering::SeqCst);
    let second_url = auth.start_sign_in().await.expect("second authorize url");
    let second_state = query_param(&second_url, "state").expect("second state");
    let callback_request = tokio::spawn(reqwest::get(format!(
        "{callback}?code=in-flight&state={second_state}"
    )));
    tokio::time::timeout(
        Duration::from_secs(5),
        edge.state.exchange_started.notified(),
    )
    .await
    .expect("exchange did not start");

    auth.sign_out();
    edge.state.release_exchange.notify_one();
    let canceled_exchange = callback_request
        .await
        .expect("callback task")
        .expect("in-flight callback response");

    assert_eq!(canceled_exchange.status().as_u16(), 409);
    assert_eq!(auth.state(), AuthState::SignedOut);
    assert!(!dir.path().join("session.json").exists());
    assert_eq!(edge.state.exchanges.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn detect_probes_edge_dev_mode() {
    // A stub that reports auth:"dev" forces dev mode even with a client id configured.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            if read_request(&mut stream).await.is_some() {
                respond(&mut stream, "200 OK", r#"{"ok":true,"auth":"dev"}"#).await;
            }
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = workos_config(&format!("http://127.0.0.1:{port}"), dir.path());
    config.dev_user_id = "dev-w".into();
    let auth = Auth::detect(config).await;
    assert!(!auth.workos_enabled(), "edge dev mode wins");
    assert_eq!(auth.access_token().await.as_deref(), Ok("dev-w"));
    task.abort();
}

// ---------------------------------------------------------------------------
// Browser sign-in routing
// ---------------------------------------------------------------------------

#[test]
fn terminal_sign_in_uses_the_browser_only_on_a_local_desktop() {
    use zuse_engine::TerminalSignIn::{Browser, PasteCode};
    let route = |no_browser, os, vars: &[&str]| {
        zuse_engine::TerminalSignIn::for_environment(no_browser, os, |name| vars.contains(&name))
    };
    assert_eq!(route(false, "macos", &[]), Browser);
    assert_eq!(route(false, "windows", &[]), Browser);
    assert_eq!(route(false, "linux", &["DISPLAY"]), Browser);
    assert_eq!(route(false, "linux", &["WAYLAND_DISPLAY"]), Browser);
    // Headless Linux, SSH sessions and explicit opt-outs paste a code.
    assert_eq!(route(false, "linux", &[]), PasteCode);
    assert_eq!(route(false, "macos", &["SSH_CONNECTION"]), PasteCode);
    assert_eq!(route(false, "linux", &["DISPLAY", "SSH_TTY"]), PasteCode);
    assert_eq!(route(true, "macos", &[]), PasteCode);
    assert_eq!(route(false, "macos", &["ZUSE_NO_BROWSER"]), PasteCode);
}

#[tokio::test]
async fn browser_sign_in_opens_the_loopback_redirect_and_completes_on_the_state_watch() {
    let edge = StubEdge::start().await;
    *edge.state.exchange_org.lock().expect("lock") = "org_1".into();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = workos_config(&edge.url(), dir.path());
    config.workos_api_base = edge.url();
    let auth = Auth::new(config);

    let opened = Mutex::new(None);
    let url = auth
        .start_browser_sign_in(|url| {
            *opened.lock().expect("lock") = Some(url.to_owned());
            true
        })
        .await
        .expect("browser route");
    assert_eq!(opened.lock().expect("lock").as_deref(), Some(url.as_str()));
    // AuthKit returns to this machine, never to the edge's paste-code page.
    let callback = query_param(&url, "redirect_uri")
        .expect("redirect")
        .replace("%3A", ":")
        .replace("%2F", "/");
    assert!(callback.starts_with("http://127.0.0.1:"), "{callback}");
    assert!(!url.contains("cli%2Fcallback"), "{url}");

    let mut state_rx = auth.watch_state();
    let state = query_param(&url, "state").expect("state");
    let ok = reqwest::get(format!("{callback}?code=abc&state={state}"))
        .await
        .expect("callback");
    assert_eq!(ok.status().as_u16(), 200);
    wait_for(&mut state_rx, |s| s.is_signed_in()).await;
    assert_eq!(edge.state.exchanges.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn browser_sign_in_falls_back_to_paste_code_without_loopback_or_browser() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The callback port is taken: the browser is never opened.
    let busy = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let mut config = workos_config("http://127.0.0.1:1", dir.path());
    config.callback_port = Some(busy.local_addr().expect("addr").port());
    let auth = Auth::new(config);
    let opened = AtomicBool::new(false);
    let route = auth
        .start_browser_sign_in(|_| {
            opened.store(true, Ordering::SeqCst);
            true
        })
        .await;
    assert_eq!(route, None);
    assert!(!opened.load(Ordering::SeqCst));
    assert!(
        auth.start_headless_sign_in()
            .contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fauth%2Fcli%2Fcallback")
    );

    // Nothing could open the URL.
    let dir = tempfile::tempdir().expect("tempdir");
    let edge = StubEdge::start().await;
    let mut config = workos_config(&edge.url(), dir.path());
    config.workos_api_base = edge.url();
    let auth = Auth::new(config);
    assert_eq!(auth.start_browser_sign_in(|_| false).await, None);
}

// ---------------------------------------------------------------------------
// Account browser sign-in (authorization code + PKCE over a loopback redirect)
// ---------------------------------------------------------------------------

fn account_jwt(subject: &str) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 3600;
    let claims = serde_json::json!({ "sub": subject, "exp": exp });
    format!("e30.{}.sig", base64url(claims.to_string().as_bytes()))
}

/// A stub identity service that redeems one authorization code, checking PKCE
/// against the challenge from the authorize URL.
async fn stub_identity(challenge: Arc<Mutex<String>>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let redeemed = Arc::new(AtomicUsize::new(0));
    let count = redeemed.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let Some((method, target, body)) = read_request(&mut stream).await else {
                continue;
            };
            if method == "GET" && target.starts_with("/user_management/authorize?") {
                respond(&mut stream, "200 OK", "{}").await;
                continue;
            }
            let body: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let verifier = body["code_verifier"].as_str().unwrap_or_default();
            let expected = {
                use base64::Engine as _;
                use sha2::Digest;
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(sha2::Sha256::digest(verifier.as_bytes()))
            };
            if method != "POST"
                || target != "/user_management/authenticate"
                || body["grant_type"] != "authorization_code"
                || body["code"] != "browser-code"
                || body["client_id"] != "client_test"
                || expected != *challenge.lock().expect("lock")
            {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    r#"{"error":"invalid_grant"}"#,
                )
                .await;
                continue;
            }
            count.fetch_add(1, Ordering::SeqCst);
            let response = serde_json::json!({
                "access_token": account_jwt("user_1"),
                "refresh_token": "refresh-browser",
                "user": { "id": "user_1", "email": "w@example.com" },
            });
            respond(&mut stream, "200 OK", &response.to_string()).await;
        }
    });
    (url, redeemed)
}

#[tokio::test]
async fn account_browser_sign_in_redeems_the_loopback_redirect_without_a_device_code() {
    use zuse_engine::account::Account;
    let challenge = Arc::new(Mutex::new(String::new()));
    let (identity, redeemed) = stub_identity(challenge.clone()).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let account = Account::configured(
        dir.path(),
        "https://api.example".into(),
        "client_test".into(),
        identity.clone(),
    )
    .expect("account");

    let progress = account.sign_in_in_browser().await.expect("browser sign-in");
    let url = reqwest::Url::parse(&progress.url).expect("authorize url");
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    assert_eq!(url.path(), "/user_management/authorize");
    assert_eq!(param("response_type"), "code");
    assert_eq!(param("provider"), "authkit");
    assert_eq!(param("code_challenge_method"), "S256");
    // No code to confirm: views fall back to "continue in your browser".
    let wire = serde_json::to_value(&progress).expect("json");
    assert!(wire.get("userCode").is_none(), "{wire}");
    assert_eq!(
        account.status().login.map(|login| login.url),
        Some(progress.url.clone())
    );
    *challenge.lock().expect("lock") = param("code_challenge");

    let callback = param("redirect_uri");
    assert!(callback.starts_with("http://127.0.0.1:"), "{callback}");
    let state = param("state");
    let stray = reqwest::get(format!("{callback}?code=browser-code&state=forged"))
        .await
        .expect("forged callback");
    assert_eq!(stray.status().as_u16(), 400);
    assert_eq!(redeemed.load(Ordering::SeqCst), 0);

    let mut status = account.watch();
    let done = reqwest::get(format!("{callback}?code=browser-code&state={state}"))
        .await
        .expect("callback");
    assert_eq!(done.status().as_u16(), 200);
    wait_for(&mut status, |s| s.user.is_some()).await;
    assert_eq!(redeemed.load(Ordering::SeqCst), 1);
    let status = account.status();
    assert_eq!(status.user.expect("user").email, "w@example.com");
    assert!(status.login.is_none() && status.error.is_none());
    assert_eq!(
        account.credential().await.expect("credential").subject,
        "user_1"
    );
}

#[tokio::test]
async fn account_browser_sign_in_reports_a_denied_redirect() {
    use zuse_engine::account::Account;
    let (identity, redeemed) = stub_identity(Arc::new(Mutex::new(String::new()))).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let account = Account::configured(
        dir.path(),
        "https://api.example".into(),
        "client_test".into(),
        identity,
    )
    .expect("account");
    let progress = account.sign_in_in_browser().await.expect("browser sign-in");
    let url = reqwest::Url::parse(&progress.url).expect("authorize url");
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    let mut status = account.watch();
    reqwest::get(format!(
        "{}?error=access_denied&state={}",
        param("redirect_uri"),
        param("state")
    ))
    .await
    .expect("callback");
    wait_for(&mut status, |s| s.error.is_some()).await;
    let status = account.status();
    assert_eq!(status.error.as_deref(), Some("Sign-in was denied."));
    assert!(status.user.is_none() && status.login.is_none());
    assert_eq!(redeemed.load(Ordering::SeqCst), 0);
}

/// The real provider returns this redirect when loopback registration is missing.
async fn identity_rejecting_loopback(
    hold: bool,
) -> (
    String,
    Arc<AtomicUsize>,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let devices = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (count, probed, unblock) = (devices.clone(), seen.clone(), release.clone());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (count, probed, unblock) = (count.clone(), probed.clone(), unblock.clone());
            tokio::spawn(async move {
                let Some((method, target, _)) = read_request(&mut stream).await else {
                    return;
                };
                let path = target.split('?').next().unwrap_or("");
                match (method.as_str(), path) {
                    ("GET", "/user_management/authorize") => {
                        probed.notify_one();
                        if hold {
                            unblock.notified().await;
                        }
                        let _ = stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://error.workos.com/redirect-uri-invalid?client_id=client_test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        let _ = stream.shutdown().await;
                    }
                    ("POST", "/user_management/authorize/device") => {
                        count.fetch_add(1, Ordering::SeqCst);
                        respond(&mut stream, "200 OK", r#"{"device_code":"fallback-device","user_code":"FALLBACK","verification_uri_complete":"https://verify.example/device","expires_in":60,"interval":1}"#).await;
                    }
                    _ => {
                        respond(
                            &mut stream,
                            "400 Bad Request",
                            r#"{"error":"authorization_pending"}"#,
                        )
                        .await
                    }
                }
            });
        }
    });
    (url, devices, seen, release)
}

#[tokio::test]
async fn account_browser_redirect_rejection_fails_loudly_without_device_codes() {
    let (identity, devices, _, _) = identity_rejecting_loopback(false).await;
    let dir = tempfile::tempdir().unwrap();
    let account = zuse_engine::account::Account::configured(
        dir.path(),
        "https://api.example".into(),
        "client_test".into(),
        identity,
    )
    .unwrap();
    let error = account.sign_in_in_browser().await.err().expect("rejected redirect is an error");
    assert!(matches!(error, zuse_engine::account::AccountError::RedirectRejected));
    assert_eq!(devices.load(Ordering::SeqCst), 0, "never falls back to device authorization");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let status = account.status();
    assert!(status.login.is_none());
    assert!(status.error.unwrap().contains("127.0.0.1"));
}

#[tokio::test]
async fn account_sign_out_during_redirect_probe_does_not_start_a_fallback_login() {
    let (identity, devices, seen, release) = identity_rejecting_loopback(true).await;
    let dir = tempfile::tempdir().unwrap();
    let account = zuse_engine::account::Account::configured(
        dir.path(),
        "https://api.example".into(),
        "client_test".into(),
        identity,
    )
    .unwrap();
    let pending = account.clone();
    let login = tokio::spawn(async move { pending.sign_in_in_browser().await });
    tokio::time::timeout(Duration::from_secs(2), seen.notified())
        .await
        .expect("probe received");
    account.sign_out().unwrap();
    release.notify_one();
    assert!(matches!(
        login.await.unwrap(),
        Err(zuse_engine::account::AccountError::Cancelled)
    ));
    assert_eq!(devices.load(Ordering::SeqCst), 0);
    assert!(account.status().login.is_none());
}

#[tokio::test]
async fn terminal_browser_redirect_rejection_does_not_open_the_provider_error_page() {
    let (identity, _, _, _) = identity_rejecting_loopback(false).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = workos_config("https://edge.example", dir.path());
    config.workos_api_base = identity;
    let auth = Auth::new(config);
    let opened = AtomicBool::new(false);
    let browser = auth
        .start_browser_sign_in(|_| {
            opened.store(true, Ordering::SeqCst);
            true
        })
        .await;
    assert_eq!(browser, None);
    assert!(!opened.load(Ordering::SeqCst));
    assert!(
        auth.start_headless_sign_in()
            .contains("auth%2Fcli%2Fcallback")
    );
}

#[tokio::test]
async fn terminal_sign_out_during_redirect_probe_does_not_open_a_stale_browser() {
    let (identity, _, seen, release) = identity_rejecting_loopback(true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = workos_config("https://edge.example", dir.path());
    config.workos_api_base = identity;
    let auth = Auth::new(config);
    let pending = auth.clone();
    let opened = Arc::new(AtomicBool::new(false));
    let browser_opened = opened.clone();
    let login = tokio::spawn(async move {
        pending
            .start_browser_sign_in(|_| {
                browser_opened.store(true, Ordering::SeqCst);
                true
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), seen.notified())
        .await
        .expect("probe received");
    auth.sign_out();
    release.notify_one();
    assert_eq!(login.await.unwrap(), None);
    assert!(!opened.load(Ordering::SeqCst));
    assert_eq!(auth.state(), AuthState::SignedOut);
}
