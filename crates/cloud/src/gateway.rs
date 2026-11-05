//! One leased transport supervisor per workspace. Payloads never become API history.
use crate::{error::ApiError, runtime::RuntimeControl};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use zuse_protocol::cloud_gateway::{
    Direction, Frame, LEGACY_PROTOCOL, Payload, supports_availability,
};

pub const MIGRATION: &str = include_str!("../migrations/0002_gateway_leases.sql");
const UNAVAILABLE: (u16, &str) = (4100, "workspace runtime unavailable");
const FENCED: (u16, &str) = (4101, "workspace generation changed");
const EXPIRED: (u16, &str) = (4102, "workspace authorization expired");
const PRESSURE: (u16, &str) = (4104, "workspace gateway backpressure");

#[derive(Clone)]
pub struct Grant {
    pub workspace: String,
    pub generation: i64,
    pub epoch: i64,
    pub runtime: bool,
    pub protocol: String,
    pub actor: String,
    pub permission: String,
    pub expires_at: i64,
}
pub struct Hub {
    rooms: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Weak<Room>>>>>,
    bytes: Arc<Semaphore>,
    sockets: Arc<Semaphore>,
}
impl Default for Hub {
    fn default() -> Self {
        Self::with_capacity(256, 32 * 1024 * 1024).expect("valid default gateway capacity")
    }
}
impl Hub {
    /// Queued bytes are a global routing budget, not a bound on total process RSS.
    pub fn with_capacity(sockets: usize, queued_bytes: usize) -> Result<Self, ApiError> {
        if !(1..=16_384).contains(&sockets)
            || !(zuse_protocol::cloud_gateway::MAX_PAYLOAD + 1..=1024 * 1024 * 1024)
                .contains(&queued_bytes)
        {
            return Err(ApiError::bad_request("invalid_gateway_capacity"));
        }
        Ok(Self {
            rooms: Default::default(),
            bytes: Arc::new(Semaphore::new(queued_bytes)),
            sockets: Arc::new(Semaphore::new(sockets)),
        })
    }
}
struct Buffered {
    message: Message,
    _budget: OwnedSemaphorePermit,
}
struct Peer {
    grant: Grant,
    tx: mpsc::Sender<Buffered>,
    close: watch::Sender<Option<(u16, &'static str)>>,
    pending_until: Option<i64>,
    ready: bool,
    detached: bool,
}
struct State {
    peers: HashMap<String, Peer>,
    runtime: Option<String>,
    closed: bool,
}
struct Room {
    control: Option<Arc<RuntimeControl>>,
    activity: tokio::sync::Mutex<Option<Instant>>,
    workspace: String,
    token: String,
    fence: (i64, i64),
    lease_valid_until: Mutex<Instant>,
    state: Mutex<State>,
    bytes: Arc<Semaphore>,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
impl Hub {
    pub fn permit(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        self.sockets
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::unavailable("workspace_gateway_capacity"))
    }
    pub async fn room(
        &self,
        control: Arc<RuntimeControl>,
        grant: &Grant,
    ) -> Result<Arc<SessionRoom>, ApiError> {
        let entry = {
            let mut rooms = lock(&self.rooms);
            rooms.retain(|_, entry| {
                Arc::strong_count(entry) > 1
                    || entry.try_lock().is_ok_and(|room| room.strong_count() > 0)
            });
            rooms.entry(grant.workspace.clone()).or_default().clone()
        };
        let mut slot = entry.lock().await;
        if let Some(room) = slot.upgrade() {
            if room.fence != (grant.generation, grant.epoch) || lock(&room.state).closed {
                return Err(ApiError::conflict("workspace_gateway_reconnecting"));
            }
            return Ok(Arc::new(SessionRoom(room)));
        }
        let token = uuid::Uuid::new_v4().to_string();
        let lease_requested_at = Instant::now();
        control
            .gateway_lease(&grant.workspace, &token, (grant.generation, grant.epoch))
            .await?;
        let room = Arc::new(Room {
            control: Some(control.clone()),
            activity: tokio::sync::Mutex::new(None),
            workspace: grant.workspace.clone(),
            token,
            fence: (grant.generation, grant.epoch),
            lease_valid_until: Mutex::new(lease_requested_at + Duration::from_secs(9)),
            bytes: self.bytes.clone(),
            state: Mutex::new(State {
                peers: HashMap::new(),
                runtime: None,
                closed: false,
            }),
        });
        *slot = Arc::downgrade(&room);
        let supervisor = room.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            tick.tick().await;
            loop {
                tick.tick().await;
                let room = &supervisor;
                {
                    let mut state = lock(&room.state);
                    if state.peers.is_empty() {
                        state.closed = true;
                        break;
                    }
                }
                room.expire();
                let peers: Vec<_> = {
                    lock(&room.state)
                        .peers
                        .iter()
                        .map(|(id, peer)| (id.clone(), peer.grant.clone()))
                        .collect()
                };
                let checks = futures::stream::iter(peers.into_iter().map(|(id, grant)| {
                    let control = control.clone();
                    async move { (id, control.revalidate_gateway_peer(&grant).await) }
                }))
                .buffer_unordered(16)
                .collect::<Vec<_>>();
                match tokio::time::timeout(Duration::from_secs(3), checks).await {
                    Ok(results) => {
                        for (id, result) in results {
                            if result.is_err() {
                                if let Some(peer) = lock(&room.state).peers.get(&id) {
                                    let _ = peer.close.send(Some(EXPIRED));
                                }
                            }
                        }
                    }
                    Err(_) => {
                        room.close_all(EXPIRED);
                        break;
                    }
                }
                let requested_at = Instant::now();
                let lease = tokio::time::timeout(
                    Duration::from_secs(3),
                    control.gateway_lease(&room.workspace, &room.token, room.fence),
                )
                .await;
                match lease {
                    Ok(Ok(())) => {
                        *lock(&room.lease_valid_until) = requested_at + Duration::from_secs(9);
                    }
                    failure => {
                        let transient = match &failure {
                            Err(_) => true,
                            Ok(Err(error)) => error.code == "cloud_database_unavailable",
                            _ => false,
                        };
                        if !retain_lease(transient, *lock(&room.lease_valid_until), Instant::now())
                        {
                            room.close_all(FENCED);
                            break;
                        }
                        // Never extend a lease on an ambiguous database response. Forwarding
                        // also checks this deadline, independently of the renewal task.
                        tracing::warn!("Gateway lease renewal deferred within confirmed validity");
                    }
                }
            }
            supervisor.close_all(FENCED);
            control
                .release_gateway(&supervisor.workspace, &supervisor.token)
                .await;
        });
        Ok(Arc::new(SessionRoom(room)))
    }
}
fn retain_lease(transient: bool, confirmed_until: Instant, now: Instant) -> bool {
    transient && now < confirmed_until
}

#[cfg(test)]
mod lease_tests {
    use super::*;

    #[test]
    fn configured_capacity_rejects_invalid_budgets_and_sheds_excess_sockets() {
        assert!(Hub::with_capacity(0, 32 * 1024 * 1024).is_err());
        assert!(Hub::with_capacity(16_385, 32 * 1024 * 1024).is_err());
        assert!(Hub::with_capacity(1, 1).is_err());
        let hub = Hub::with_capacity(1, 32 * 1024 * 1024).unwrap();
        let permit = hub.permit().unwrap();
        assert!(hub.permit().is_err());
        drop(permit);
        assert!(hub.permit().is_ok());
    }

    #[tokio::test]
    async fn two_hundred_machine_sockets_share_the_bounded_gateway() {
        use axum::response::IntoResponse;
        use tokio_tungstenite::tungstenite::Message as ClientMessage;
        let hub = Arc::new(Hub::default());
        let rooms = Arc::new(
            (0..200)
                .map(|i| {
                    Arc::new(SessionRoom(Arc::new(Room {
                        control: None,
                        activity: tokio::sync::Mutex::new(None),
                        workspace: format!("machine-{i}"),
                        token: "test".into(),
                        fence: (1, 1),
                        lease_valid_until: Mutex::new(Instant::now() + Duration::from_secs(60)),
                        state: Mutex::new(State {
                            peers: HashMap::new(),
                            runtime: None,
                            closed: false,
                        }),
                        bytes: hub.bytes.clone(),
                    })))
                })
                .collect::<Vec<_>>(),
        );
        let serving = hub.clone();
        let app = axum::Router::new().route(
            "/ws/{id}",
            axum::routing::get(
                move |axum::extract::Path(id): axum::extract::Path<usize>,
                      upgrade: axum::extract::WebSocketUpgrade| {
                    let hub = serving.clone();
                    let session = rooms.get(id).cloned();
                    async move {
                        let Some(session) = session else {
                            return axum::http::StatusCode::NOT_FOUND.into_response();
                        };
                        let Ok(permit) = hub.permit() else {
                            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                        };
                        upgrade.on_upgrade(move |socket| async move {
                            session
                                .serve(
                                    Grant {
                                        workspace: format!("machine-{id}"),
                                        generation: 1,
                                        epoch: 1,
                                        runtime: true,
                                        protocol: LEGACY_PROTOCOL.into(),
                                        actor: "test".into(),
                                        permission: "write".into(),
                                        expires_at: now() + 60_000,
                                    },
                                    socket,
                                    permit,
                                )
                                .await;
                        })
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let sockets = futures::future::join_all((0..200).map(|i| async move {
            let (mut socket, _) =
                tokio_tungstenite::connect_async(format!("ws://{address}/ws/{i}"))
                    .await
                    .unwrap();
            socket
                .send(ClientMessage::Ping(vec![1, 2, 3]))
                .await
                .unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(reply, ClientMessage::Pong(vec![1, 2, 3]));
            socket
        }))
        .await;
        assert_eq!(hub.sockets.available_permits(), 56);
        assert_eq!(sockets.len(), 200);
        drop(sockets);
        tokio::time::timeout(Duration::from_secs(5), async {
            while hub.sockets.available_permits() != 256 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(hub.bytes.available_permits(), 32 * 1024 * 1024);
        server.abort();
    }

    #[test]
    fn transient_failure_cannot_extend_ownership_or_mask_fencing() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(9);
        assert!(retain_lease(true, deadline, now));
        assert!(!retain_lease(false, deadline, now));
        assert!(!retain_lease(true, deadline, deadline));
        assert!(!retain_lease(
            true,
            deadline,
            deadline + Duration::from_millis(1)
        ));
    }

    #[tokio::test]
    async fn socket_writer_answers_ping_and_delivers_confirmed_fencing() {
        let room = Arc::new(Room {
            control: None,
            activity: tokio::sync::Mutex::new(None),
            workspace: "test".into(),
            token: "test".into(),
            fence: (1, 1),
            lease_valid_until: Mutex::new(Instant::now() + Duration::from_secs(60)),
            state: Mutex::new(State {
                peers: HashMap::new(),
                runtime: None,
                closed: false,
            }),
            bytes: Arc::new(Semaphore::new(1024 * 1024)),
        });
        let session = Arc::new(SessionRoom(room.clone()));
        let app = axum::Router::new().route(
            "/ws",
            axum::routing::get(move |upgrade: axum::extract::WebSocketUpgrade| {
                let session = session.clone();
                async move {
                    upgrade.on_upgrade(move |socket| async move {
                        let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
                        session
                            .serve(
                                Grant {
                                    workspace: "test".into(),
                                    generation: 1,
                                    epoch: 1,
                                    runtime: true,
                                    protocol: LEGACY_PROTOCOL.into(),
                                    actor: "test".into(),
                                    permission: "write".into(),
                                    expires_at: now() + 60_000,
                                },
                                socket,
                                permit,
                            )
                            .await;
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .unwrap();
        use tokio_tungstenite::tungstenite::Message as ClientMessage;
        socket
            .send(ClientMessage::Ping(b"alive".to_vec()))
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(reply, ClientMessage::Pong(b"alive".to_vec()));
        room.close_all(FENCED);
        let reply = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(reply, ClientMessage::Close(Some(frame)) if u16::from(frame.code) == FENCED.0)
        );
        server.abort();
    }
}
// Public wrapper prevents access to routing state outside the socket owner.
pub struct SessionRoom(Arc<Room>);
impl SessionRoom {
    async fn forward_received(&self, id: &str, payload: Payload) {
        let grant = {
            let state = lock(&self.0.state);
            state
                .peers
                .get(id)
                .filter(|peer| {
                    !state.closed
                        && state.runtime.is_some()
                        && peer.ready
                        && !peer.detached
                        && !peer.grant.runtime
                        && peer.grant.permission == "edit"
                        && peer.grant.protocol == zuse_protocol::cloud_gateway::RUST_PROTOCOL
                })
                .map(|peer| peer.grant.clone())
        };
        // Runtime output and passive transports remain opaque. Only a client
        // editing this Rust task has request method metadata inspected.
        if let (Some(grant), Some(control)) = (grant, &self.0.control) {
            if crate::runtime::activity::meaningful_rpc(&payload) {
                // Serialize the first write after quiet so concurrent clients
                // cannot forward around an uncommitted inactivity update.
                let mut last = self.0.activity.lock().await;
                if last.is_none_or(|at| {
                    at.elapsed()
                        >= Duration::from_millis(crate::runtime::activity::COALESCE_MS as u64)
                }) {
                    let recorded = tokio::time::timeout(
                        Duration::from_secs(2),
                        control.record_gateway_activity(&grant, now()),
                    )
                    .await;
                    if !matches!(recorded, Ok(Ok(true))) {
                        // The request was never forwarded. Close rather than
                        // silently discard it; mutation retry policy stays at
                        // the existing client layer, never blind server replay.
                        if let Some(peer) = lock(&self.0.state).peers.get(id) {
                            let _ = peer.close.send(Some(UNAVAILABLE));
                        }
                        return;
                    }
                    *last = Some(Instant::now());
                }
            }
        }
        self.0.forward(id, payload);
    }

    pub async fn serve(
        self: Arc<Self>,
        grant: Grant,
        socket: WebSocket,
        _permit: OwnedSemaphorePermit,
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        // Initial chat/files/Git subscriptions arrive together; bytes remain globally bounded.
        let (tx, mut rx) = mpsc::channel(128);
        let (close, mut closed) = watch::channel(None);
        self.0.attach(
            id.clone(),
            Peer {
                grant,
                tx,
                close,
                pending_until: None,
                ready: false,
                detached: false,
            },
        );
        let _connection = ConnectionGuard {
            room: self.0.clone(),
            id: id.clone(),
        };
        let (mut sink, mut source) = socket.split();
        let (controls, mut control_rx) = mpsc::channel(8);
        let mut writer_closed = closed.clone();
        let mut writer = SocketWriter(tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    biased;
                    changed = writer_closed.changed() => {
                        if changed.is_ok() {
                            let value = *writer_closed.borrow();
                            if let Some((code, reason)) = value {
                                let _ = tokio::time::timeout(Duration::from_secs(1), sink.send(
                                    Message::Close(Some(CloseFrame { code, reason: reason.into() }))
                                )).await;
                            }
                        }
                        return;
                    }
                    control = control_rx.recv() => {
                        let Some(control) = control else { return; };
                        control
                    }
                    out = rx.recv() => {
                        let Some(out) = out else { return; };
                        // Keep the byte-budget permit alive until the write completes.
                        if !matches!(tokio::time::timeout(Duration::from_secs(10), sink.send(out.message)).await, Ok(Ok(()))) {
                            tracing::debug!(reason = "write_timeout_or_error", "Gateway socket closed");
                            return;
                        }
                        continue;
                    }
                };
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(2), sink.send(message)).await,
                    Ok(Ok(()))
                ) {
                    tracing::debug!(
                        reason = "control_write_timeout_or_error",
                        "Gateway socket closed"
                    );
                    return;
                }
            }
        }));
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        let mut last_received = Instant::now();
        loop {
            tokio::select! {
                _ = &mut writer.0 => break,
                changed = closed.changed() => { if changed.is_err() { break; } },
                _ = heartbeat.tick() => {
                    if last_received.elapsed() >= Duration::from_secs(45) {
                        tracing::debug!(reason = "heartbeat_timeout", "Gateway socket closed");
                        break;
                    }
                    if controls.try_send(Message::Ping(Vec::new().into())).is_err() { break; }
                }
                incoming = source.next() => {
                    last_received = Instant::now();
                    match incoming {
                        Some(Ok(Message::Text(text))) => self.forward_received(&id, Payload::Text(text.to_string())).await,
                        Some(Ok(Message::Binary(bytes))) => self.forward_received(&id, Payload::Binary(bytes.to_vec())).await,
                        // Axum/tungstenite automatically queues the matching Pong.
                        // Sending another from the independent writer duplicates it.
                        Some(Ok(Message::Ping(_))) => {},
                        Some(Ok(Message::Pong(_))) => {},
                        Some(Ok(Message::Close(_))) => break,
                        _ => {
                            tracing::debug!(reason = "read_error_or_eof", "Gateway socket closed");
                            break;
                        }
                    }
                }
            }
        }
    }
}
// Canceling a request must retire its writer rather than detach a socket task.
struct SocketWriter(tokio::task::JoinHandle<()>);
impl Drop for SocketWriter {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct ConnectionGuard {
    room: Arc<Room>,
    id: String,
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.room.detach(&self.id);
    }
}
impl Room {
    fn send(&self, peer: &Peer, message: Message) -> bool {
        let size = match &message {
            Message::Text(s) => s.len(),
            Message::Binary(b) => b.len(),
            _ => 0,
        };
        let permit = self.bytes.clone().try_acquire_many_owned((size + 1) as u32);
        let ok = match permit {
            Ok(budget) => peer
                .tx
                .try_send(Buffered {
                    message,
                    _budget: budget,
                })
                .is_ok(),
            Err(_) => false,
        };
        if !ok {
            let _ = peer.close.send(Some(PRESSURE));
        }
        ok
    }
    fn control(&self, peer: &Peer, value: serde_json::Value) -> bool {
        self.send(peer, Message::Text(value.to_string().into()))
    }
    fn pending(&self, peer: &mut Peer) {
        if !supports_availability(&peer.grant.protocol) {
            let _ = peer.close.send(Some(UNAVAILABLE));
            return;
        }
        peer.pending_until = Some(peer.pending_until.unwrap_or_else(|| now() + 30000));
        self.control(
            peer,
            json!({"_zuseGateway":"availability","state":"pending"}),
        );
    }
    fn open(&self, runtime: &Peer, id: &str, client: &mut Peer) {
        if client.detached {
            return;
        }
        if supports_availability(&client.grant.protocol) && client.ready {
            self.control(
                client,
                json!({"_zuseGateway":"availability","state":"available","reset":true}),
            );
            client.detached = true;
            client.pending_until = None;
            return;
        }
        let opened=self.control(runtime,json!({"type":"client.open","connectionId":id,"actorId":client.grant.actor,"permission":client.grant.permission}));
        if opened {
            if supports_availability(&client.grant.protocol) {
                self.control(
                    client,
                    json!({"_zuseGateway":"availability","state":"available","reset":false}),
                );
            }
            client.ready = true;
            client.pending_until = None;
        } else {
            let _ = client.close.send(Some(PRESSURE));
        }
    }
    fn attach(&self, id: String, mut peer: Peer) {
        let mut state = lock(&self.state);
        if state.closed
            || *lock(&self.lease_valid_until) <= Instant::now()
            || peer.grant.expires_at <= now()
        {
            let _ = peer.close.send(Some(EXPIRED));
            return;
        }
        if state.peers.len() >= 65 {
            let _ = peer.close.send(Some(PRESSURE));
            return;
        }
        if peer.grant.runtime {
            if let Some(old) = state.runtime.take().and_then(|id| state.peers.remove(&id)) {
                let _ = old.close.send(Some((4001, "runtime replaced")));
            }
            for (client_id, client) in &mut state.peers {
                if !client.grant.runtime {
                    if !supports_availability(&client.grant.protocol) && client.ready {
                        let _ = client.close.send(Some(UNAVAILABLE));
                    } else {
                        self.open(&peer, client_id, client);
                    }
                }
            }
            state.runtime = Some(id.clone());
        } else if let Some(runtime) = state.runtime.as_ref().and_then(|id| state.peers.get(id)) {
            self.open(runtime, &id, &mut peer);
        } else {
            self.pending(&mut peer);
        }
        state.peers.insert(id, peer);
    }
    fn forward(&self, id: &str, payload: Payload) {
        let state = lock(&self.state);
        if state.closed || *lock(&self.lease_valid_until) <= Instant::now() {
            return;
        }
        let Some(peer) = state.peers.get(id) else {
            return;
        };
        if peer.detached {
            return;
        }
        if peer.grant.runtime {
            if state.runtime.as_deref() != Some(id) {
                return;
            }
            // Only runtime-directed binary frames are routable upward. Runtime text
            // cannot inject control messages or impersonate client routing authority.
            let frame = if peer.grant.protocol == LEGACY_PROTOCOL {
                decode_legacy(payload)
            } else {
                match payload {
                    Payload::Binary(bytes) => Frame::decode(&bytes),
                    _ => None,
                }
            };
            let Some(frame) = frame else {
                return;
            };
            if frame.direction != Direction::Runtime {
                return;
            }
            if let Some(client) = state
                .peers
                .get(&frame.connection_id)
                .filter(|p| !p.grant.runtime && !p.detached)
            {
                let message = match frame.payload {
                    Payload::Text(s) => Message::Text(s.into()),
                    Payload::Binary(b) => Message::Binary(b.into()),
                };
                self.send(client, message);
            }
        } else if let Some(runtime) = state.runtime.as_ref().and_then(|id| state.peers.get(id)) {
            let frame = Frame {
                direction: Direction::Client,
                connection_id: id.into(),
                payload,
            };
            let encoded = if runtime.grant.protocol == LEGACY_PROTOCOL {
                encode_legacy(&frame)
            } else {
                frame.encode().map(|b| Message::Binary(b.into()))
            };
            if let Some(message) = encoded {
                self.send(runtime, message);
            } else {
                let _ = peer.close.send(Some(PRESSURE));
            }
        }
        // Pending transports hold no application frames. The client retries its
        // connection after availability; mutations are never buffered or replayed.
    }
    fn detach(&self, id: &str) {
        let mut state = lock(&self.state);
        let Some(peer) = state.peers.remove(id) else {
            return;
        };
        if state.closed {
            return;
        }
        if peer.grant.runtime && state.runtime.as_deref() == Some(id) {
            state.runtime = None;
            for client in state.peers.values_mut() {
                if !client.grant.runtime {
                    self.pending(client);
                }
            }
        } else if !peer.grant.runtime
            && !peer.detached
            && let Some(runtime) = state.runtime.as_ref().and_then(|id| state.peers.get(id))
        {
            self.control(runtime, json!({"type":"client.close","connectionId":id}));
        }
    }
    fn expire(&self) {
        let state = lock(&self.state);
        for peer in state.peers.values() {
            if peer.pending_until.is_some_and(|at| at <= now()) {
                let _ = peer.close.send(Some(UNAVAILABLE));
            }
        }
    }
    fn close_all(&self, reason: (u16, &'static str)) {
        let mut state = lock(&self.state);
        state.closed = true;
        for peer in state.peers.values() {
            let _ = peer.close.send(Some(reason));
        }
    }
}

fn encode_legacy(frame: &Frame) -> Option<Message> {
    if frame.connection_id.is_empty() || frame.connection_id.len() > 512 {
        return None;
    }
    let (encoding, payload) = match &frame.payload {
        Payload::Text(s) if s.len() <= zuse_protocol::cloud_gateway::MAX_PAYLOAD => {
            ("text", s.clone())
        }
        Payload::Binary(b) if b.len() <= zuse_protocol::cloud_gateway::MAX_PAYLOAD => {
            ("base64", STANDARD.encode(b))
        }
        _ => return None,
    };
    Some(Message::Text(json!({"type":"client.frame","connectionId":frame.connection_id,"encoding":encoding,"payload":payload}).to_string().into()))
}
fn decode_legacy(payload: Payload) -> Option<Frame> {
    let Payload::Text(text) = payload else {
        return None;
    };
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    if value["type"] != "runtime.frame" {
        return None;
    }
    let connection = value["connectionId"].as_str()?;
    if connection.is_empty() || connection.len() > 512 {
        return None;
    }
    let payload = value["payload"].as_str()?;
    let payload = match value["encoding"].as_str()? {
        "text" if payload.len() <= zuse_protocol::cloud_gateway::MAX_PAYLOAD => {
            Payload::Text(payload.into())
        }
        "base64" => {
            let bytes = STANDARD.decode(payload).ok()?;
            if bytes.len() > zuse_protocol::cloud_gateway::MAX_PAYLOAD {
                return None;
            }
            Payload::Binary(bytes)
        }
        _ => return None,
    };
    Some(Frame {
        direction: Direction::Runtime,
        connection_id: connection.into(),
        payload,
    })
}
