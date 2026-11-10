//! Opaque browser sessions for an explicitly configured cloud-only controller.
//! The factory owns account state; this transport never accepts a bearer in a URL.
use axum::{
    Router,
    extract::{State, WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use zuse_rpc::RpcService;

const COOKIE: &str = "__Host-zuse_cloud_session";
const CHANGED: &str = "x-zuse-browser-session-changed";

pub struct SessionService {
    pub rpc: Arc<dyn RpcService>,
    pub retire: Arc<dyn Fn() + Send + Sync>,
    pub authenticated: Arc<dyn Fn() -> bool + Send + Sync>,
    pub closed: CancellationToken,
}
pub trait Factory: Send + Sync {
    fn create(&self, public_controller_id: &str) -> anyhow::Result<SessionService>;
}
struct Entry {
    service: SessionService,
    expires: tokio::time::Instant,
    anonymous_until: tokio::time::Instant,
    closed: CancellationToken,
    sockets: Arc<Semaphore>,
    retired: AtomicBool,
}
impl Entry {
    fn retire(&self) {
        if !self.retired.swap(true, Ordering::AcqRel) {
            self.closed.cancel();
            (self.service.retire)();
        }
    }

    fn deadline(&self) -> tokio::time::Instant {
        if (self.service.authenticated)() {
            self.expires
        } else {
            self.expires.min(self.anonymous_until)
        }
    }
    fn expired(&self) -> bool {
        self.closed.is_cancelled() || self.deadline() <= tokio::time::Instant::now()
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.retire();
    }
}
#[derive(Clone)]
pub struct HostedBrowser {
    origin: Arc<str>,
    factory: Arc<dyn Factory>,
    sessions: Arc<Mutex<HashMap<String, Arc<Entry>>>>,
    slots: Arc<Semaphore>,
    max_sessions: usize,
    lifetime: Duration,
    shutdown: CancellationToken,
}
impl HostedBrowser {
    pub fn new(
        origin: &str,
        factory: Arc<dyn Factory>,
        max_sessions: usize,
        lifetime: Duration,
    ) -> anyhow::Result<Self> {
        let uri: axum::http::Uri = origin.parse()?;
        anyhow::ensure!(
            uri.scheme_str() == Some("https")
                && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
                && uri.path() == "/"
                && uri.query().is_none()
                && !origin.ends_with('/'),
            "hosted browser requires an exact HTTPS origin"
        );
        anyhow::ensure!(
            (1..=32).contains(&max_sessions)
                && lifetime >= Duration::from_secs(60)
                && lifetime <= Duration::from_secs(86400),
            "invalid hosted browser bounds"
        );
        Ok(Self {
            origin: origin.into(),
            factory,
            sessions: Arc::default(),
            slots: Arc::new(Semaphore::new(8)),
            max_sessions,
            lifetime,
            shutdown: CancellationToken::new(),
        })
    }
    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }
    pub fn router(self) -> Router {
        Router::new()
            .route("/api/session/check", post(check_session))
            .route("/api/session", axum::routing::delete(logout))
            .route("/api/rpc", get(socket))
            .with_state(self)
    }
    fn valid_origin(&self, headers: &HeaderMap) -> bool {
        headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) == Some(self.origin.as_ref())
            && !headers
                .get("sec-fetch-site")
                .and_then(|h| h.to_str().ok())
                .is_some_and(|v| v != "same-origin" && v != "none")
    }
    fn cookie_token(headers: &HeaderMap) -> Option<&str> {
        let mut matches = headers
            .get(header::COOKIE)?
            .to_str()
            .ok()?
            .split(';')
            .filter_map(|c| c.trim().strip_prefix(&format!("{COOKIE}=")));
        let value = matches.next()?;
        (matches.next().is_none()
            && value.len() == 64
            && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(value)
    }
    fn prune(&self, sessions: &mut HashMap<String, Arc<Entry>>) {
        sessions.retain(|_, entry| {
            if entry.expired() {
                entry.retire();
                false
            } else {
                true
            }
        });
    }
    fn lookup(&self, headers: &HeaderMap) -> Option<Arc<Entry>> {
        let mut sessions = self.sessions.lock().ok()?;
        self.prune(&mut sessions);
        sessions.get(Self::cookie_token(headers)?).cloned()
    }
    fn establish(
        &self,
        headers: &HeaderMap,
    ) -> Result<(Arc<Entry>, Option<String>, bool), StatusCode> {
        if self.shutdown.is_cancelled() {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        self.prune(&mut sessions);
        if let Some(entry) = Self::cookie_token(headers).and_then(|token| sessions.get(token)) {
            return Ok((entry.clone(), None, false));
        }
        if sessions.len() >= self.max_sessions {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        // Public device identity follows the existing browser draft UUID contract;
        // the separate cookie above remains the private admission secret.
        let public_id = uuid::Uuid::new_v4().to_string();
        let service = self
            .factory
            .create(&public_id)
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let entry = Arc::new(Entry {
            expires: tokio::time::Instant::now() + self.lifetime,
            anonymous_until: tokio::time::Instant::now() + Duration::from_secs(600),
            closed: service.closed.clone(),
            service,
            sockets: Arc::new(Semaphore::new(2)),
            retired: AtomicBool::new(false),
        });
        sessions.insert(token.clone(), entry.clone());
        Ok((entry, Some(token), true))
    }
}
fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}
fn busy() -> Response {
    private((StatusCode::TOO_MANY_REQUESTS,[(header::RETRY_AFTER,"3")],
        axum::Json(serde_json::json!({"error":"controller_busy","message":"Close an unused browser tab and retry shortly."}))).into_response())
}
async fn check_session(State(server): State<HostedBrowser>, headers: HeaderMap) -> Response {
    if !server.valid_origin(&headers) {
        return private(StatusCode::FORBIDDEN.into_response());
    }
    if server.slots.available_permits() == 0 {
        return busy();
    }
    match server.establish(&headers) {
        Err(status) => private(status.into_response()),
        Ok((entry, cookie, changed)) => {
            if entry.sockets.available_permits() == 0 {
                return busy();
            }
            // This admits only the factory's account-only anonymous surface;
            // successful bootstrap is not evidence of a signed-in account.
            let mut response =
                axum::Json(serde_json::json!({"controllerReady":true})).into_response();
            if let Some(cookie) = cookie {
                response.headers_mut().insert(
                    header::SET_COOKIE,
                    HeaderValue::from_str(&format!(
                        "{COOKIE}={cookie}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
                        server.lifetime.as_secs()
                    ))
                    .unwrap(),
                );
            }
            if changed {
                response
                    .headers_mut()
                    .insert(CHANGED, HeaderValue::from_static("1"));
            }
            private(response)
        }
    }
}
async fn logout(State(server): State<HostedBrowser>, headers: HeaderMap) -> Response {
    if !server.valid_origin(&headers) {
        return private(StatusCode::FORBIDDEN.into_response());
    }
    if let Some(token) = HostedBrowser::cookie_token(&headers) {
        if let Ok(mut sessions) = server.sessions.lock() {
            if let Some(entry) = sessions.remove(token) {
                entry.retire();
            }
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-zuse_cloud_session=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0",
        ),
    );
    response
        .headers_mut()
        .insert(CHANGED, HeaderValue::from_static("1"));
    private(response)
}
async fn socket(
    State(server): State<HostedBrowser>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !server.valid_origin(&headers) {
        return private(StatusCode::FORBIDDEN.into_response());
    }
    let Some(entry) = server.lookup(&headers) else {
        return private(StatusCode::UNAUTHORIZED.into_response());
    };
    let Ok(global) = server.slots.clone().try_acquire_owned() else {
        return private(StatusCode::TOO_MANY_REQUESTS.into_response());
    };
    let Ok(local) = entry.sockets.clone().try_acquire_owned() else {
        return private(StatusCode::TOO_MANY_REQUESTS.into_response());
    };
    ws.max_message_size(2 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            let (_global, _local) = (global, local);
            // An idle socket expires without requiring another HTTP probe.
            let closed = entry.closed.clone();
            let shutdown = server.shutdown.clone();
            let service = entry.service.rpc.clone();
            crate::rpc_socket(
                service,
                socket,
                async move {
                    loop {
                        tokio::select! {
                            _ = closed.cancelled() => break,
                            _ = shutdown.cancelled() => break,
                            _ = tokio::time::sleep_until(entry.deadline()) => {
                                if entry.expired() { entry.retire(); break; }
                            }
                        }
                    }
                },
                crate::browser_io::Compression::None,
                false,
            )
            .await;
        })
        .into_response()
}

// Methods to add to HostedBrowser only after the frozen backend capture releases.
impl HostedBrowser {
    pub fn router_with_assets(self, assets: std::path::PathBuf) -> anyhow::Result<Router> {
        use std::io::Read;
        use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};
        let assets = assets.canonicalize()?;
        let index = assets.join("index.html");
        anyhow::ensure!(
            index.canonicalize()?.starts_with(&assets),
            "Browser index must remain in its compiled asset directory"
        );
        let mut html = String::new();
        std::fs::File::open(index)?
            .take(1024 * 1024 + 1)
            .read_to_string(&mut html)?;
        anyhow::ensure!(
            html.len() <= 1024 * 1024 && (html.contains("'/app/") || html.contains("\"/app/")),
            "Compile the hosted browser with Trunk --public-url /app/"
        );
        Ok(self
            .router()
            .route(
                "/app",
                get(|| async { axum::response::Redirect::temporary("/app/") }),
            )
            .nest_service("/app/", ServeDir::new(assets))
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
            )))
    }
    /// Uses the same exact HTTPS router for an owned local controller fixture.
    pub async fn serve_tls(
        self,
        listener: tokio::net::TcpListener,
        assets: std::path::PathBuf,
        tls: crate::TlsConfig,
    ) -> anyhow::Result<()> {
        let shutdown = self.shutdown.clone();
        let _guard = shutdown.clone().drop_guard();
        axum::serve(
            crate::tls::Listener::new(listener, tls),
            self.router_with_assets(assets)?,
        )
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "hosted_tests.rs"]
mod hosted_tests;
