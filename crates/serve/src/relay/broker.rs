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
    serve::ListenerExt,
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
    sync::{Notify, Semaphore, oneshot},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower_http::set_header::SetResponseHeaderLayer;
use zuse_remote::{MAX_RECORD_BYTES, PairId};
struct Waiting {
    id: uuid::Uuid,
    client: oneshot::Sender<WebSocket>,
}
#[derive(Default)]
struct Routes {
    waiting: HashMap<String, VecDeque<Waiting>>,
    hosts: HashMap<String, usize>,
}
#[derive(Clone)]
pub struct RelayBroker {
    token: HostToken,
    routes: Arc<Mutex<Routes>>,
    changed: Arc<Notify>,
    pending_clients: Arc<Semaphore>,
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
        let mut routes = self.broker.routes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(queue) = routes.waiting.get_mut(&self.pair) {
            queue.retain(|waiting| waiting.id != self.id && !waiting.client.is_closed());
            if queue.is_empty() {
                routes.waiting.remove(&self.pair);
            }
        }
        if let Some(count) = routes.hosts.get_mut(&self.pair) {
            *count -= 1;
            if *count == 0 {
                routes.hosts.remove(&self.pair);
            }
        }
        drop(routes);
        self.broker.changed.notify_waiters();
    }
}
impl RelayBroker {
    pub fn new(token: HostToken) -> Self {
        Self {
            token,
            routes: Default::default(),
            changed: Arc::new(Notify::new()),
            pending_clients: Arc::new(Semaphore::new(64)),
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
            .fallback_service(crate::assets::router(assets))
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
                axum::serve(listener.tap_io(crate::no_delay), router)
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
    // Queue only the HTTP upgrade, before any encrypted application request.
    // A known live host can replenish its standby pool while a burst waits.
    let Ok(_pending) = broker.pending_clients.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let waiter = loop {
        let changed = broker.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        {
            let mut routes = broker.routes.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(queue) = routes.waiting.get_mut(&pair) {
                queue.retain(|waiting| !waiting.client.is_closed());
                let waiter = queue.pop_front();
                if queue.is_empty() {
                    routes.waiting.remove(&pair);
                }
                if let Some(waiter) = waiter {
                    break waiter;
                }
            }
            if !routes.hosts.contains_key(&pair) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        }
        tokio::select! { biased;
            _ = broker.shutdown.cancelled() => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            _ = tokio::time::sleep_until(deadline) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            _ = changed => {},
        }
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
        let mut routes = broker.routes.lock().unwrap_or_else(|e| e.into_inner());
        let queue = routes.waiting.entry(pair.clone()).or_default();
        queue.retain(|waiting| !waiting.client.is_closed());
        if queue.len() >= 8 {
            return;
        }
        queue.push_back(Waiting { id, client: tx });
        *routes.hosts.entry(pair.clone()).or_default() += 1;
    }
    broker.changed.notify_waiters();
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::SinkExt;
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{self, client::IntoClientRequest},
    };
    async fn host_socket(
        origin: &str,
        pair: &str,
        token: &HostToken,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("{origin}/api/paired-host/{pair}")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", token.header().parse().unwrap());
        connect_async(request).await.unwrap().0
    }
    async fn pending(broker: &RelayBroker) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while broker.pending_clients.available_permits() != 63 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn burst_upgrade_waits_for_replacement_times_out_and_releases_on_shutdown() {
        let root = tempfile::tempdir().unwrap();
        let token = HostToken::parse(crate::BrowserServer::random_token()).unwrap();
        let broker = RelayBroker::new(token.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("ws://{}", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let task = tokio::spawn(broker.clone().serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ));
        let pair = PairId::generate().unwrap();
        let mut first_host = host_socket(&origin, pair.as_str(), &token).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if broker
                    .routes
                    .lock()
                    .unwrap()
                    .waiting
                    .contains_key(pair.as_str())
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let url = format!("{origin}/api/paired-rpc/{}", pair.as_str());
        let first = connect_async(&url).await.unwrap().0;
        let queued = tokio::spawn({
            let url = url.clone();
            async move { connect_async(url).await }
        });
        pending(&broker).await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_millis(5001)).await;
        tokio::time::resume();
        assert!(
            matches!(queued.await.unwrap(),Err(tungstenite::Error::Http(response)) if response.status().as_u16()==503)
        );
        assert_eq!(broker.pending_clients.available_permits(), 64);
        let queued = tokio::spawn({
            let url = url.clone();
            async move { connect_async(url).await }
        });
        pending(&broker).await;
        let mut replacement = host_socket(&origin, pair.as_str(), &token).await;
        let mut second = queued.await.unwrap().unwrap().0;
        second
            .send(tungstenite::Message::Binary(vec![1, 2, 3]))
            .await
            .unwrap();
        loop {
            match tokio::time::timeout(Duration::from_secs(2), replacement.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                tungstenite::Message::Binary(bytes) => {
                    assert_eq!(bytes, vec![1, 2, 3]);
                    break;
                }
                tungstenite::Message::Ping(_)
                | tungstenite::Message::Pong(_)
                | tungstenite::Message::Text(_) => {}
                _ => panic!("replacement disconnected"),
            }
        }
        assert_eq!(broker.pending_clients.available_permits(), 64);
        let queued = tokio::spawn(async move { connect_async(url).await });
        pending(&broker).await;
        stop.cancel();
        assert!(queued.await.unwrap().is_err());
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(broker.pending_clients.available_permits(), 64);
        assert!(broker.routes.lock().unwrap().hosts.is_empty());
        drop(first);
        drop(second);
        let _ = first_host.close(None).await;
        let _ = replacement.close(None).await;
    }
}
