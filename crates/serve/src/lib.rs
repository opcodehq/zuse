//! Authenticated browser transport over the engine's canonical RPC dispatch.

use axum::{
    Json, Router,
    extract::{State, WebSocketUpgrade, ws::Message},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
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

#[derive(Clone)]
pub struct BrowserServer {
    service: Arc<dyn RpcService>,
    token: Arc<str>,
    public_origin: Option<String>,
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
            anyhow::ensure!(
                origin.starts_with("https://") && !origin.ends_with('/'),
                "public origin must be an HTTPS origin without a trailing slash"
            );
        }
        Ok(Self {
            service,
            token: token.into(),
            public_origin,
        })
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
            .route("/api/rpc", get(socket))
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
        axum::serve(listener, self.router(assets))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
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
        origin == format!("http://{host}") || origin == format!("https://{host}")
    }

    fn cookie(&self, value: &str) -> String {
        let secure = if self.public_origin.is_some() {
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

async fn socket(
    State(server): State<BrowserServer>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !server.authorized(&headers) || !server.valid_origin(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.max_message_size(2 * 1024 * 1024).on_upgrade(move |socket| async move {
        let (mut writer, mut reader) = socket.split();
        let (out_tx, mut out_rx) = mpsc::channel::<String>(128);
        let (in_tx, in_rx) = mpsc::channel::<String>(128);
        let dispatch = tokio::spawn(zuse_rpc::serve_connection(server.service, out_tx, in_rx));
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
        drop(in_tx);
        let _ = dispatch.await;
    })
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
