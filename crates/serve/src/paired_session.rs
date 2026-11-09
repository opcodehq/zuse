//! Paired sessions that survive their socket.
//!
//! The dispatcher, its subscriptions and the unacknowledged replies belong to
//! the session, so a client can move between LAN, tailnet and relay sockets
//! without its work noticing. Every socket still performs its own Noise
//! handshake and live authorization check before attaching.
use super::paired::{Authorized, encrypted, send};
use crate::BrowserServer;
use axum::extract::ws::Message;
use futures::{Sink, Stream, StreamExt};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use zuse_remote::{
    SecureChannel,
    authority::Credentials,
    heartbeat,
    session::{self, AttachReply, AttachRequest, Control, Resume},
};
use zuse_rpc::RpcReply;

const MAX_SESSIONS: usize = 128;
const MAX_SESSIONS_PER_PAIR: usize = 8;

struct Live {
    id: String,
    pair: String,
    created: Instant,
    in_tx: mpsc::Sender<String>,
    out_rx: tokio::sync::Mutex<mpsc::Receiver<String>>,
    resume: Mutex<Resume>,
    /// Held by the socket currently serving this session.
    exchange: tokio::sync::Mutex<()>,
    takeover: Mutex<CancellationToken>,
    attached: watch::Sender<bool>,
    stop: CancellationToken,
    _dispatch: AbortOnDropHandle<()>,
}
impl Live {
    fn resume(&self) -> std::sync::MutexGuard<'_, Resume> {
        self.resume.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Shared by every listener and relay connector of one profile, so a session
/// opened on one route can continue on another.
#[derive(Default)]
pub struct PairedSessions {
    live: Mutex<HashMap<String, Arc<Live>>>,
    closed: CancellationToken,
}
impl PairedSessions {
    /// End every session. Listeners coming and going do not; only the owner does.
    pub fn close(&self) {
        self.closed.cancel();
    }
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Live>>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn len(&self) -> usize {
        self.map().len()
    }
    pub fn is_empty(&self) -> bool {
        self.map().is_empty()
    }
    fn find(&self, id: &str, pair: &str) -> Option<Arc<Live>> {
        self.map().get(id).filter(|live| live.pair == pair).cloned()
    }
    fn remove(&self, id: &str) {
        self.map().remove(id);
    }
    /// Reclaim the oldest idle session rather than refuse a working client.
    fn admit(&self, live: Arc<Live>) -> bool {
        let mut map = self.map();
        let pair_count = map.values().filter(|l| l.pair == live.pair).count();
        if pair_count >= MAX_SESSIONS_PER_PAIR || map.len() >= MAX_SESSIONS {
            let same_pair = pair_count >= MAX_SESSIONS_PER_PAIR;
            let oldest = map
                .values()
                .filter(|l| !same_pair || l.pair == live.pair)
                .filter(|l| !*l.attached.borrow())
                .min_by_key(|l| l.created)
                .cloned();
            let Some(oldest) = oldest else {
                return false;
            };
            oldest.stop.cancel();
            map.remove(&oldest.id);
        }
        map.insert(live.id.clone(), live);
        true
    }
}

fn open(
    server: &BrowserServer,
    device: String,
    credentials: &Arc<Credentials>,
) -> Option<Arc<Live>> {
    let (out_tx, out_rx) = mpsc::channel::<String>(16);
    let (in_tx, in_rx) = mpsc::channel::<String>(16);
    let service = Arc::new(Authorized {
        inner: server.service.clone(),
        credentials: credentials.clone(),
        device,
    });
    let dispatch = AbortOnDropHandle::new(tokio::spawn(zuse_rpc::serve_connection(
        service, out_tx, in_rx,
    )));
    let live = Arc::new(Live {
        id: session::generate_session_id().ok()?,
        pair: credentials.id().as_str().into(),
        created: Instant::now(),
        in_tx,
        out_rx: tokio::sync::Mutex::new(out_rx),
        resume: Mutex::default(),
        exchange: tokio::sync::Mutex::new(()),
        takeover: Mutex::new(CancellationToken::new()),
        attached: watch::Sender::new(true),
        stop: CancellationToken::new(),
        _dispatch: dispatch,
    });
    if !server.paired_sessions.admit(live.clone()) {
        return None;
    }
    tokio::spawn(expire(
        server.paired_sessions.clone(),
        live.clone(),
        credentials.clone(),
    ));
    Some(live)
}

async fn expire(sessions: Arc<PairedSessions>, live: Arc<Live>, credentials: Arc<Credentials>) {
    let mut attached = live.attached.subscribe();
    let hold = Duration::from_millis(session::HOST_HOLD_MS);
    let watch = async {
        loop {
            let _ = attached.wait_for(|attached| !*attached).await;
            let resumed = tokio::time::timeout(hold, attached.wait_for(|attached| *attached));
            if resumed.await.is_err() {
                return;
            }
        }
    };
    tokio::select! { biased;
        _ = sessions.closed.cancelled() => {},
        _ = credentials.revoked() => {},
        _ = live.stop.cancelled() => {},
        _ = watch => {},
    }
    sessions.remove(&live.id);
    live.takeover
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cancel();
}

async fn engine_info(server: &BrowserServer) -> Option<serde_json::Value> {
    match server
        .service
        .handle(zuse_rpc::methods::ENGINE_INFO, serde_json::json!({}))
        .await
    {
        Ok(RpcReply::Value(value)) => Some(value),
        _ => None,
    }
}

fn attach_reply(reply: &AttachReply) -> Option<Vec<u8>> {
    serde_json::to_vec(&zuse_rpc::ServerFrame {
        id: session::ATTACH_ID,
        ok: Some(serde_json::to_value(reply).ok()?),
        ..Default::default()
    })
    .ok()
}

pub(super) fn attach_request(text: &str) -> Option<AttachRequest> {
    let frame = serde_json::from_str::<zuse_rpc::ClientFrame>(text).ok()?;
    if frame.id != session::ATTACH_ID || frame.method.as_deref() != Some(session::ATTACH_METHOD) {
        return None;
    }
    serde_json::from_value(frame.params).ok()
}

pub(super) async fn run<R, W, E>(
    server: BrowserServer,
    device: String,
    credentials: Arc<Credentials>,
    mut channel: SecureChannel,
    mut reader: R,
    mut writer: W,
    request: AttachRequest,
) where
    R: Stream<Item = Result<Message, E>> + Unpin,
    W: Sink<Message> + Unpin,
{
    let requested = request.session.clone();
    let existing = requested
        .as_deref()
        .and_then(|id| server.paired_sessions.find(id, credentials.id().as_str()));
    if requested.is_some() && existing.is_none() {
        // The old session is gone. Say so instead of silently starting over:
        // the client must report which requests may have been lost.
        let refused = AttachReply::default();
        if let Some(reply) = attach_reply(&refused) {
            let _ = encrypted(&mut channel, &mut writer, &reply).await;
        }
        channel.close();
        return;
    }
    let resumed = existing.is_some();
    let Some(live) = existing.or_else(|| open(&server, device, &credentials)) else {
        channel.close();
        return;
    };
    // Stop the previous socket before reading counters it may still change.
    let takeover = {
        let next = CancellationToken::new();
        let mut current = live.takeover.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *current, next.clone()).cancel();
        next
    };
    let exclusive = tokio::select! { biased;
        _ = takeover.cancelled() => { channel.close(); return; }
        guard = live.exchange.lock() => guard,
    };
    let compress = request.caps.iter().any(|cap| cap == session::CAP_LZ4);
    let (received, replay) = {
        let mut resume = live.resume();
        let replay = if resumed {
            resume.rewind(request.received)
        } else {
            Ok(vec![])
        };
        (resume.received(), replay)
    };
    let Ok(replay) = replay else {
        // A count outside what was sent means the peers no longer agree.
        live.stop.cancel();
        channel.close();
        return;
    };
    live.attached.send_replace(true);
    let mut routes = server.advertised_routes.clone();
    let reply = AttachReply {
        session: live.id.clone(),
        resumed,
        received,
        caps: if compress {
            vec![session::CAP_LZ4.into()]
        } else {
            vec![]
        },
        engine_info: engine_info(&server).await,
        routes: routes.borrow_and_update().clone(),
    };
    let exchange = async {
        let reply = attach_reply(&reply).ok_or(())?;
        encrypted(&mut channel, &mut writer, &reply).await?;
        for wire in replay {
            encrypted(&mut channel, &mut writer, &wire).await?;
        }
        let mut last_encrypted = Instant::now();
        let mut ack_tick = tokio::time::interval(Duration::from_millis(session::ACK_INTERVAL_MS));
        let mut out_rx = live.out_rx.lock().await;
        loop {
            let can_send = live.resume().can_send();
            tokio::select! {
                _ = tokio::time::sleep_until(last_encrypted + Duration::from_secs(90)) => return Err(()),
                _ = ack_tick.tick() => {
                    let ack = live.resume().pending_ack();
                    if let Some(ack) = ack {
                        encrypted(&mut channel, &mut writer, &ack).await?;
                    }
                },
                changed = routes.changed() => {
                    if changed.is_ok() {
                        let frame = session::routes_frame(&routes.borrow_and_update()).map_err(|_| ())?;
                        encrypted(&mut channel, &mut writer, &frame).await?;
                    }
                },
                data = out_rx.recv(), if can_send => {
                    let Some(data) = data else { return Ok::<_, ()>(()); };
                    let wire = session::encode_message(data.as_bytes(), compress);
                    live.resume().record_sent(wire.clone());
                    encrypted(&mut channel, &mut writer, &wire).await?;
                },
                frame = reader.next() => match frame {
                    Some(Ok(Message::Binary(record))) => {
                        let Some(bytes) = channel.open(&record).map_err(|_| ())? else { continue; };
                        last_encrypted = Instant::now();
                        if let Some(reply) = heartbeat::response(&bytes).map_err(|_| ())? {
                            if !credentials.is_live() { return Err(()); }
                            encrypted(&mut channel, &mut writer, &reply).await?;
                            continue;
                        }
                        match session::parse_control(&bytes).map_err(|_| ())? {
                            Some(Control::Ack(count)) => {
                                live.resume().acknowledge(count).map_err(|_| ())?;
                                continue;
                            }
                            Some(Control::Routes(_) | Control::End) => return Err(()),
                            None => {}
                        }
                        let size = bytes.len();
                        let decoded = session::decode_message(bytes).map_err(|_| ())?;
                        let payload = String::from_utf8(decoded).map_err(|_| ())?;
                        // Count only after delivery: a socket cancelled here
                        // leaves the message for the client to send again.
                        tokio::time::timeout(Duration::from_secs(30), live.in_tx.send(payload))
                            .await
                            .map_err(|_| ())?
                            .map_err(|_| ())?;
                        let ack = live.resume().record_received(size);
                        if let Some(ack) = ack {
                            encrypted(&mut channel, &mut writer, &ack).await?;
                        }
                    },
                    Some(Ok(Message::Ping(data))) => send(&mut writer, Message::Pong(data)).await?,
                    Some(Ok(Message::Pong(_))) => {},
                    _ => return Ok(()),
                }
            }
        }
    };
    let ended = tokio::select! { biased;
        _ = server.paired_sessions.closed.cancelled() => true,
        _ = credentials.revoked() => true,
        // A listener sharing its sessions can go away while others stay.
        _ = server.shutdown.cancelled() => server.owns_sessions,
        _ = takeover.cancelled() => false,
        _ = exchange => false,
    };
    if ended {
        // Nothing can resume this; spare the client its retries.
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            encrypted(&mut channel, &mut writer, &session::end_frame()),
        )
        .await;
    }
    channel.close();
    drop(exclusive);
    if !takeover.is_cancelled() {
        live.attached.send_replace(false);
    }
}
