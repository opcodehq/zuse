use super::HostToken;
use crate::TlsConfig;
use axum::{
    Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures::StreamExt;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, oneshot},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};
use zuse_remote::{MAX_RECORD_BYTES, PairId};
struct Waiting {
    id: uuid::Uuid,
    client: oneshot::Sender<WebSocket>,
}
#[derive(Clone)]
pub struct RelayBroker {
    token: HostToken,
    waiting: Arc<Mutex<HashMap<String, VecDeque<Waiting>>>>,
    slots: Arc<Semaphore>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}
// A cancelled upgrade or socket must not leave a route claiming availability.
struct Registration {
    broker: RelayBroker,
    pair: String,
    id: uuid::Uuid,
}
impl Drop for Registration {
    fn drop(&mut self) {
        let mut routes = self
            .broker
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(queue) = routes.get_mut(&self.pair) {
            queue.retain(|waiting| waiting.id != self.id && !waiting.client.is_closed());
            if queue.is_empty() {
                routes.remove(&self.pair);
            }
        }
    }
}
impl RelayBroker {
    pub fn new(token: HostToken) -> Self {
        Self {
            token,
            waiting: Default::default(),
            slots: Arc::new(Semaphore::new(256)),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }
    pub fn router(self, assets: PathBuf) -> Router {
        Router::new()
            .route("/health", get(|| async { "Zuse relay" }))
            .route("/api/paired-host/{pair_id}", get(host))
            .route("/api/paired-rpc/{pair_id}", get(client))
            .fallback_service(
                ServeDir::new(&assets).not_found_service(ServeFile::new(assets.join("index.html"))),
            )
            .with_state(self)
            .layer(SetResponseHeaderLayer::overriding(
                header::X_CONTENT_TYPE_OPTIONS,
                header::HeaderValue::from_static("nosniff"),
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::REFERRER_POLICY,
                header::HeaderValue::from_static("no-referrer"),
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::HeaderName::from_static("cross-origin-opener-policy"),
                header::HeaderValue::from_static("same-origin"),
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::HeaderName::from_static("cross-origin-embedder-policy"),
                header::HeaderValue::from_static("require-corp"),
            ))
    }
    pub async fn serve_with_shutdown<F>(
        self,
        listener: TcpListener,
        assets: PathBuf,
        tls: Option<TlsConfig>,
        signal: F,
    ) -> anyhow::Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        let _guard = shutdown.clone().drop_guard();
        let tasks = self.tasks.clone();
        let signal = async move {
            signal.await;
            shutdown.cancel();
        };
        let router = self.router(assets);
        match tls {
            Some(tls) => {
                axum::serve(crate::tls::Listener::new(listener, tls), router)
                    .with_graceful_shutdown(signal)
                    .await?
            }
            None => {
                axum::serve(listener, router)
                    .with_graceful_shutdown(signal)
                    .await?
            }
        }
        tasks.close();
        tasks.wait().await;
        Ok(())
    }
}
async fn host(
    State(broker): State<RelayBroker>,
    Path(pair): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let valid = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| broker.token.matches(value));
    if !valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if PairId::parse(&pair).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(permit) = broker.slots.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let tasks = broker.tasks.clone();
    ws.max_message_size(MAX_RECORD_BYTES)
        .on_upgrade(move |socket| {
            tasks.track_future(async move{
        let _permit=permit;let shutdown=broker.shutdown.clone();
        tokio::select!{biased;_=shutdown.cancelled()=>{},_=wait_for_client(broker,pair,socket)=>{}}
    })
        })
}
async fn client(
    State(broker): State<RelayBroker>,
    Path(pair): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    if broker.shutdown.is_cancelled() || PairId::parse(&pair).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let waiter = {
        let mut routes = broker.waiting.lock().unwrap_or_else(|e| e.into_inner());
        let Some(queue) = routes.get_mut(&pair) else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        queue.retain(|waiting| !waiting.client.is_closed());
        let waiter = queue.pop_front();
        if queue.is_empty() {
            routes.remove(&pair);
        }
        waiter
    };
    let Some(waiter) = waiter else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(MAX_RECORD_BYTES)
        .on_upgrade(move |socket| async move {
            let _ = waiter.client.send(socket);
        })
}
async fn wait_for_client(broker: RelayBroker, pair: String, mut host: WebSocket) {
    let id = uuid::Uuid::new_v4();
    let (tx, rx) = oneshot::channel();
    {
        let mut routes = broker.waiting.lock().unwrap_or_else(|e| e.into_inner());
        let queue = routes.entry(pair.clone()).or_default();
        queue.retain(|waiting| !waiting.client.is_closed());
        if queue.len() >= 8 {
            return;
        }
        queue.push_back(Waiting { id, client: tx });
    }
    let _registration = Registration { broker, pair, id };
    let ready=async{
        let mut ping=tokio::time::interval(Duration::from_secs(15));let mut last_seen=tokio::time::Instant::now();
        tokio::pin!(rx);
        loop {tokio::select!{
            client=&mut rx=>return client.map_err(|_|()),
            _=ping.tick()=>{if last_seen.elapsed()>Duration::from_secs(45){return Err(());}send(&mut host,Message::Ping(vec![].into())).await?;},
            message=host.next()=>match message {
                Some(Ok(Message::Pong(_)))=>last_seen=tokio::time::Instant::now(),
                Some(Ok(Message::Ping(data)))=>{last_seen=tokio::time::Instant::now();send(&mut host,Message::Pong(data)).await?;},
                _=>return Err(()),
            }
        }}
    }.await;
    let Ok(mut client) = ready else {
        return;
    };
    if send(&mut host, Message::Text("connected".into()))
        .await
        .is_err()
    {
        return;
    }
    let exchange = async {
        let mut ping = tokio::time::interval(Duration::from_secs(15));
        let mut host_seen = tokio::time::Instant::now();
        let mut client_seen = host_seen;
        loop {
            tokio::select! {
                _=ping.tick()=>{
                    if host_seen.elapsed()>Duration::from_secs(60)||client_seen.elapsed()>Duration::from_secs(60){return Err(());}
                    send(&mut host,Message::Ping(vec![].into())).await?;send(&mut client,Message::Ping(vec![].into())).await?;
                },
                message=host.next()=>{host_seen=tokio::time::Instant::now();match message {
                    Some(Ok(Message::Binary(data)))=>send(&mut client,Message::Binary(data)).await?,
                    Some(Ok(Message::Ping(data)))=>send(&mut host,Message::Pong(data)).await?,Some(Ok(Message::Pong(_)))=>{},_=>return Err(())
                }},
                message=client.next()=>{client_seen=tokio::time::Instant::now();match message {
                    Some(Ok(Message::Binary(data)))=>send(&mut host,Message::Binary(data)).await?,
                    Some(Ok(Message::Ping(data)))=>send(&mut client,Message::Pong(data)).await?,Some(Ok(Message::Pong(_)))=>{},_=>return Err(())
                }}
            }
        }
    };
    let _: Result<(), ()> = exchange.await;
}
async fn send(socket: &mut WebSocket, message: Message) -> Result<(), ()> {
    tokio::time::timeout(Duration::from_secs(5), socket.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}
