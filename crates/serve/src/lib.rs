//! Authenticated browser transport over the engine's canonical RPC dispatch.

use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State, WebSocketUpgrade, ws::Message},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::mpsc};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};
use zuse_rpc::RpcService;
mod access_token;
mod paired;
pub mod relay;
mod tls;
pub use tls::TlsConfig;

#[derive(Clone)]
pub struct BrowserServer {
    service: Arc<dyn RpcService>,
    pairings: Option<(zuse_remote::authority::Authority, String)>,
    paired_slots: Arc<tokio::sync::Semaphore>,
    token: Arc<str>,
    public_origin: Option<String>,
    secure_transport: bool,
    shutdown: tokio_util::sync::CancellationToken,
    connections: tokio_util::task::TaskTracker,
}

impl BrowserServer {
    pub fn new(
        service: Arc<dyn RpcService>,
        token: String,
        public_origin: Option<String>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            token.len() >= 32,
            "browser access token must contain at least 32 characters"
        );
        if let Some(origin) = &public_origin {
            let parsed: axum::http::Uri = origin.parse()?;
            anyhow::ensure!(
                parsed.scheme_str() == Some("https")
                    && parsed
                        .authority()
                        .is_some_and(|a| !a.as_str().contains('@'))
                    && parsed.path() == "/"
                    && parsed.query().is_none()
                    && !origin.ends_with('/'),
                "public origin must be an HTTPS origin without a trailing slash"
            );
        }
        Ok(Self {
            service,
            pairings: None,
            paired_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            token: token.into(),
            secure_transport: public_origin.is_some(),
            public_origin,
            shutdown: tokio_util::sync::CancellationToken::new(),
            connections: tokio_util::task::TaskTracker::new(),
        })
    }

    /// Share the runtime owner's cancellation authority with upgraded sessions.
    pub fn with_shutdown_token(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.shutdown = token;
        self
    }

    pub fn with_pairings(
        mut self,
        authority: zuse_remote::authority::Authority,
        device_id: impl Into<String>,
    ) -> Self {
        self.pairings = Some((authority, device_id.into()));
        self
    }

    pub fn load_or_create_token(directory: &std::path::Path) -> anyhow::Result<String> {
        access_token::load_or_create(directory)
    }

    pub fn random_token() -> String {
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    }

    pub fn router(self, assets: PathBuf) -> Router {
        Router::new()
            .route(
                "/health",
                get(|| async { Json(serde_json::json!({"status":"ok", "service":"zuse"})) }),
            )
            .route("/api/session", post(unlock).delete(lock))
            .route("/api/session/check", post(session_status))
            .route("/api/rpc", get(socket))
            .route("/api/paired-rpc/{pair_id}", get(paired::upgrade))
            .route("/api/attachments", post(upload))
            .fallback_service(
                ServeDir::new(&assets).not_found_service(ServeFile::new(assets.join("index.html"))),
            )
            .with_state(self)
            .layer(SetResponseHeaderLayer::overriding(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::HeaderName::from_static("cross-origin-opener-policy"),
                HeaderValue::from_static("same-origin"),
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::HeaderName::from_static("cross-origin-embedder-policy"),
                HeaderValue::from_static("require-corp"),
            ))
    }

    pub async fn serve(self, listener: TcpListener, assets: PathBuf) -> anyhow::Result<()> {
        self.serve_with_shutdown(listener, assets, None, async {
            let _ = shutdown_signal().await;
        })
        .await
    }

    pub async fn serve_tls(
        self,
        listener: TcpListener,
        assets: PathBuf,
        tls: TlsConfig,
    ) -> anyhow::Result<()> {
        self.serve_with_shutdown(listener, assets, Some(tls), async {
            let _ = shutdown_signal().await;
        })
        .await
    }

    /// Embed the browser server with the host's shutdown authority. Cancelling
    /// the host also closes upgraded sockets and their RPC subscriptions.
    pub async fn serve_with_shutdown<F>(
        mut self,
        listener: TcpListener,
        assets: PathBuf,
        tls: Option<TlsConfig>,
        signal: F,
    ) -> anyhow::Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.secure_transport |= tls.is_some();
        let shutdown = self.shutdown.clone();
        let _guard = shutdown.clone().drop_guard();
        let connections = self.connections.clone();
        let signal = async move {
            signal.await;
            shutdown.cancel();
        };
        let router = self.router(assets);
        match tls {
            Some(tls) => {
                axum::serve(tls::Listener::new(listener, tls), router)
                    .with_graceful_shutdown(signal)
                    .await?
            }
            None => {
                axum::serve(listener, router)
                    .with_graceful_shutdown(signal)
                    .await?
            }
        }
        connections.close();
        connections.wait().await;
        Ok(())
    }

    fn valid_token(&self, token: &str) -> bool {
        self.token.as_bytes().ct_eq(token.as_bytes()).into()
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        if let Some(value) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            return self.valid_token(value);
        }
        headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|cookies| {
                cookies
                    .split(';')
                    .filter_map(|v| v.trim().strip_prefix("zuse_session="))
                    .any(|token| self.valid_token(token))
            })
    }

    fn valid_origin(&self, headers: &HeaderMap) -> bool {
        let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        if let Some(expected) = &self.public_origin {
            return origin == expected;
        }
        let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let scheme = if self.secure_transport {
            "https"
        } else {
            "http"
        };
        origin == format!("{scheme}://{host}")
    }

    fn cookie(&self, value: &str) -> String {
        let secure = if self.secure_transport {
            "; Secure"
        } else {
            ""
        };
        format!("zuse_session={value}; Path=/; HttpOnly; SameSite=Strict{secure}")
    }
}

#[derive(Deserialize)]
struct Unlock {
    token: String,
}

async fn unlock(
    State(server): State<BrowserServer>,
    headers: HeaderMap,
    Json(input): Json<Unlock>,
) -> Response {
    if !server.valid_origin(&headers) || !server.valid_token(&input.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    (
        [(header::SET_COOKIE, server.cookie(&input.token))],
        Json(serde_json::json!({"authenticated":true})),
    )
        .into_response()
}

async fn lock(State(server): State<BrowserServer>, headers: HeaderMap) -> Response {
    if !server.valid_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    (
        [(
            header::SET_COOKIE,
            format!("{}; Max-Age=0", server.cookie("")),
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

async fn session_status(State(server): State<BrowserServer>, headers: HeaderMap) -> Response {
    let status = if server.authorized(&headers) && server.valid_origin(&headers) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::UNAUTHORIZED
    };
    ([(header::CACHE_CONTROL, "no-store")], status).into_response()
}

async fn socket(
    State(server): State<BrowserServer>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !server.authorized(&headers) || !server.valid_origin(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let connections = server.connections.clone();
    ws.max_message_size(2 * 1024 * 1024)
        .on_upgrade(move |socket| connections.track_future(browser_socket(server, socket)))
}

async fn browser_socket(server: BrowserServer, socket: axum::extract::ws::WebSocket) {
    // An upgrade completing after listener shutdown must not start RPC work.
    if server.shutdown.is_cancelled() {
        return;
    }
    let (mut writer, mut reader) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(128);
    let (in_tx, in_rx) = mpsc::channel::<String>(128);
    let dispatch = tokio::spawn(zuse_rpc::serve_connection(server.service, out_tx, in_rx));
    let shutdown = server.shutdown;
    {
        let exchange = async {
            loop {
                tokio::select! {
                    payload = out_rx.recv() => match payload {
                        Some(payload) => if writer.send(Message::Text(payload.into())).await.is_err() { break; },
                        None => break,
                    },
                    message = reader.next() => match message {
                        Some(Ok(Message::Text(payload))) => if in_tx.send(payload.to_string()).await.is_err() { break; },
                        Some(Ok(Message::Ping(data))) => if writer.send(Message::Pong(data)).await.is_err() { break; },
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {},
                    }
                }
            }
        };
        tokio::select! { biased; _ = shutdown.cancelled() => {}, _ = exchange => {} }
    }
    drop(in_tx);
    let _ = dispatch.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    use zuse_rpc::{RpcError, RpcReply};

    struct FixtureService;
    #[async_trait::async_trait]
    impl RpcService for FixtureService {
        async fn handle(&self, _: &str, _: serde_json::Value) -> Result<RpcReply, RpcError> {
            RpcReply::value(&serde_json::json!({"fixture":true}))
        }
    }

    fn server() -> BrowserServer {
        BrowserServer::new(Arc::new(FixtureService), "a".repeat(64), None).unwrap()
    }

    #[tokio::test]
    async fn session_probe_requires_current_cookie_and_origin_without_reissuing_credentials() {
        for (cookie, origin, expected) in [
            (
                format!("zuse_session={}", "a".repeat(64)),
                "http://127.0.0.1:27656",
                StatusCode::NO_CONTENT,
            ),
            (
                format!("zuse_session={}", "b".repeat(64)),
                "http://127.0.0.1:27656",
                StatusCode::UNAUTHORIZED,
            ),
            (
                format!("zuse_session={}", "a".repeat(64)),
                "https://foreign.example",
                StatusCode::UNAUTHORIZED,
            ),
        ] {
            let response = server()
                .router(PathBuf::from("missing-fixture-assets"))
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/session/check")
                        .header("host", "127.0.0.1:27656")
                        .header("origin", origin)
                        .header("cookie", cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }

    #[tokio::test]
    async fn cross_origin_unlock_never_sets_an_authenticated_cookie() {
        let app = server().router(PathBuf::from("missing-fixture-assets"));
        let request = Request::builder()
            .method("POST")
            .uri("/api/session")
            .header("host", "127.0.0.1:27656")
            .header("origin", "https://attacker.example")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"token":"a".repeat(64)}).to_string(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }

    #[tokio::test]
    async fn authenticated_same_origin_unlock_issues_http_only_cookie() {
        let app = server().router(PathBuf::from("missing-fixture-assets"));
        let request = Request::builder()
            .method("POST")
            .uri("/api/session")
            .header("host", "127.0.0.1:27656")
            .header("origin", "http://127.0.0.1:27656")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({"token":"a".repeat(64)}).to_string(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        assert_eq!(
            response.headers()["cross-origin-opener-policy"],
            "same-origin"
        );
        assert_eq!(
            response.headers()["cross-origin-embedder-policy"],
            "require-corp"
        );
    }

    #[test]
    fn a_valid_cookie_does_not_authorize_a_different_origin() {
        let server = server();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:27656".parse().unwrap());
        headers.insert(header::ORIGIN, "https://attacker.example".parse().unwrap());
        headers.insert(
            header::COOKIE,
            format!("zuse_session={}", "a".repeat(64)).parse().unwrap(),
        );
        assert!(server.authorized(&headers));
        assert!(!server.valid_origin(&headers));
    }
}

#[derive(Deserialize)]
struct UploadName {
    name: String,
}

/// Raw file bytes are staged through the same bounded upload ledger as native
/// clients. Keep chunks aligned so concatenating their base64 is lossless.
async fn upload(
    State(server): State<BrowserServer>,
    Query(input): Query<UploadName>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    use zuse_protocol::methods;
    if !server.authorized(&headers) || !server.valid_origin(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if input.name.is_empty() || input.name.len() > 1024 {
        return (StatusCode::BAD_REQUEST, "Invalid filename").into_response();
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut data = body.into_data_stream();
    let mut carry = Vec::new();
    let mut total = 0usize;
    let mut seq = 0u64;
    while let Some(chunk) = data.next().await {
        let Ok(chunk) = chunk else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        total = total.saturating_add(chunk.len());
        if total > 32 * 1024 * 1024 {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        carry.extend_from_slice(&chunk);
        while carry.len() >= 196608 {
            let bytes = carry.drain(..196608).collect::<Vec<_>>();
            if server
                .service
                .handle(
                    methods::UPLOAD_CHUNK,
                    serde_json::json!({"uploadId":id,"data":STANDARD.encode(bytes),"seq":seq}),
                )
                .await
                .is_err()
            {
                return (StatusCode::BAD_GATEWAY, "Upload failed").into_response();
            }
            seq += 1;
        }
    }
    if !carry.is_empty() || seq == 0 {
        if server
            .service
            .handle(
                methods::UPLOAD_CHUNK,
                serde_json::json!({"uploadId":id,"data":STANDARD.encode(carry),"seq":seq}),
            )
            .await
            .is_err()
        {
            return (StatusCode::BAD_GATEWAY, "Upload failed").into_response();
        }
    }
    match server
        .service
        .handle(
            methods::UPLOAD_COMMIT,
            serde_json::json!({"uploadId":id,"fileName":input.name}),
        )
        .await
    {
        Ok(zuse_rpc::RpcReply::Value(value)) => Json(value).into_response(),
        _ => (StatusCode::BAD_GATEWAY, "Could not commit attachment").into_response(),
    }
}

#[cfg(test)]
mod upload_tests {
    use super::*;
    use async_trait::async_trait;
    use axum::http::Request;
    use std::sync::Mutex;
    use tower::ServiceExt;
    use zuse_rpc::{RpcError, RpcReply};

    #[derive(Default)]
    struct UploadLedger {
        chunks: Mutex<Vec<(u64, String)>>,
        committed: Mutex<Option<Vec<u8>>>,
    }
    #[async_trait]
    impl RpcService for UploadLedger {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            match method {
                zuse_protocol::methods::UPLOAD_CHUNK => {
                    self.chunks.lock().unwrap().push((
                        params["seq"].as_u64().unwrap(),
                        params["data"].as_str().unwrap().into(),
                    ));
                    RpcReply::value(&serde_json::json!({"ok":true}))
                }
                zuse_protocol::methods::UPLOAD_COMMIT => {
                    let mut chunks = self.chunks.lock().unwrap().clone();
                    chunks.sort_by_key(|(seq, _)| *seq);
                    let encoded = chunks.into_iter().map(|(_, data)| data).collect::<String>();
                    *self.committed.lock().unwrap() = Some(STANDARD.decode(encoded).unwrap());
                    RpcReply::value(&serde_json::json!({"path":"uploads/fixture/image.png"}))
                }
                _ => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn browser_upload_preserves_bytes_across_arbitrary_http_chunks() {
        let ledger = Arc::new(UploadLedger::default());
        let server = BrowserServer::new(ledger.clone(), "a".repeat(64), None).unwrap();
        let bytes = (0..500_003).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let chunks = bytes
            .chunks(100_001)
            .map(|c| Ok::<_, std::io::Error>(c.to_vec()))
            .collect::<Vec<_>>();
        let request = Request::builder()
            .method("POST")
            .uri("/api/attachments?name=image.png")
            .header("host", "127.0.0.1:27656")
            .header("origin", "http://127.0.0.1:27656")
            .header("cookie", format!("zuse_session={}", "a".repeat(64)))
            .body(Body::from_stream(futures::stream::iter(chunks)))
            .unwrap();
        let response = server
            .router(PathBuf::new())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            ledger.committed.lock().unwrap().as_deref(),
            Some(bytes.as_slice())
        );
    }

    #[tokio::test]
    async fn foreign_origin_upload_cannot_write_to_the_engine() {
        let ledger = Arc::new(UploadLedger::default());
        let server = BrowserServer::new(ledger.clone(), "a".repeat(64), None).unwrap();
        let request = Request::builder()
            .method("POST")
            .uri("/api/attachments?name=payload.txt")
            .header("host", "127.0.0.1:27656")
            .header("origin", "https://foreign.example")
            .header("cookie", format!("zuse_session={}", "a".repeat(64)))
            .body(Body::from("untrusted content"))
            .unwrap();
        let response = server
            .router(PathBuf::new())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(ledger.chunks.lock().unwrap().is_empty());
        assert!(ledger.committed.lock().unwrap().is_none());
    }
}

/// Drain the engine on service-manager termination as well as interactive exit.
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result=tokio::signal::ctrl_c()=>result, _=terminate.recv()=>Ok(()) }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
