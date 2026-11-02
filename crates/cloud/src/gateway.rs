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
        Self {
            rooms: Default::default(),
            bytes: Arc::new(Semaphore::new(32 * 1024 * 1024)),
            sockets: Arc::new(Semaphore::new(256)),
        }
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
                let requested_at = Instant::now();
                let lease = tokio::time::timeout(
                    Duration::from_secs(3),
                    control.gateway_lease(&room.workspace, &room.token, room.fence),
                )
                .await;
                if !matches!(lease, Ok(Ok(()))) {
                    room.close_all(FENCED);
                    break;
                }
                *lock(&room.lease_valid_until) = requested_at + Duration::from_secs(9);
            }
            supervisor.close_all(FENCED);
            control
                .release_gateway(&supervisor.workspace, &supervisor.token)
                .await;
        });
        Ok(Arc::new(SessionRoom(room)))
    }
}
// Public wrapper prevents access to routing state outside the socket owner.
pub struct SessionRoom(Arc<Room>);
impl SessionRoom {
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
        loop {
            tokio::select! {
                changed=closed.changed()=> {
                    if changed.is_ok() {
                        let value=*closed.borrow();
                        if let Some((code,reason))=value { let _=tokio::time::timeout(Duration::from_secs(1),sink.send(Message::Close(Some(CloseFrame{code,reason:reason.into()})))).await; }
                    }
                    break;
                }
                incoming=source.next()=>match incoming {
                    Some(Ok(Message::Text(text)))=>self.0.forward(&id,Payload::Text(text.to_string())),
                    Some(Ok(Message::Binary(bytes)))=>self.0.forward(&id,Payload::Binary(bytes.to_vec())),
                    Some(Ok(Message::Ping(bytes)))=> { if tokio::time::timeout(Duration::from_secs(2),sink.send(Message::Pong(bytes))).await.is_err() {break;} },
                    Some(Ok(Message::Pong(_)))=>{}, _=>break,
                },
                out=rx.recv()=> {
                    let Some(out)=out else {break;};
                    if !matches!(tokio::time::timeout(Duration::from_secs(2),sink.send(out.message)).await,Ok(Ok(()))) { break; }
                }
            }
        }
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
