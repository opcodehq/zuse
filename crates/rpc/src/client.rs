//! Client side: request/stream multiplexing over string frames + the WebSocket dialer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{ClientFrame, RpcError, ServerFrame};

/// Per-stream queue depth. Bounded: route_frame awaits a full queue, pausing
/// the connection reader — transport backpressure instead of unbounded growth
/// when a consumer stalls behind a fast producer (watch frames every 120ms
/// during streaming used to pile up whole-transcript payloads here).
const STREAM_QUEUE_CAP: usize = 256;

enum Pending {
    Call(oneshot::Sender<Result<serde_json::Value, RpcError>>),
    Stream(mpsc::Sender<serde_json::Value>),
    Forward {
        frames: mpsc::Sender<ServerFrame>,
        readiness_ack: bool,
    },
    CheckedStream {
        items: mpsc::Sender<serde_json::Value>,
        ready: Option<oneshot::Sender<Result<(), RpcError>>>,
        drop_when_full: bool,
    },
}

pub(super) struct Shared {
    pending: Mutex<HashMap<u64, Pending>>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub(super) fn interrupt(&self, reconnecting: bool) {
        let drained: Vec<_> = self.lock().drain().collect();
        for (id, entry) in drained {
            match entry {
                Pending::Call(tx) => {
                    let error = if reconnecting {
                        RpcError::Transport("The connection closed before a reply arrived. This action may have completed; check its current state before retrying.".into())
                    } else {
                        RpcError::Closed
                    };
                    let _ = tx.send(Err(error));
                }
                Pending::Forward { frames, .. } => {
                    let _ = frames.try_send(ServerFrame {
                        id, err: Some(if reconnecting {
                            "The connection closed before completion. This action may have completed; check its current state before retrying.".into()
                        } else { "Connection closed.".into() }), ..Default::default()
                    });
                }
                Pending::CheckedStream {
                    ready: Some(ready), ..
                } => {
                    let _ = ready.send(Err(RpcError::Closed));
                }
                _ => {}
            }
        }
    }
}
#[derive(Clone)]
enum Outbound {
    Fixed(super::reconnect::Session),
    Reconnect(Arc<super::reconnect::State>),
}
impl Outbound {
    fn snapshot(&self) -> Result<super::reconnect::Session, RpcError> {
        match self {
            Self::Fixed(out) => {
                if out.retired.is_cancelled() {
                    Err(RpcError::Closed)
                } else {
                    Ok(out.clone())
                }
            }
            Self::Reconnect(state) => state.sender().ok_or_else(|| {
                RpcError::Transport(
                    "The remote engine is reconnecting. This request was not sent.".into(),
                )
            }),
        }
    }
}
pub(super) struct Incoming {
    pub body: String,
    pub retired: tokio_util::sync::CancellationToken,
}
enum Inbox {
    Fixed {
        input: mpsc::Receiver<String>,
        retired: tokio_util::sync::CancellationToken,
    },
    Reconnect(mpsc::Receiver<Incoming>),
}
impl Inbox {
    async fn recv(&mut self) -> Option<Incoming> {
        match self {
            Self::Fixed { input, retired } => match input.recv().await {
                Some(body) => Some(Incoming {
                    body,
                    retired: retired.clone(),
                }),
                None => None,
            },
            Self::Reconnect(input) => input.recv().await,
        }
    }
}

/// A multiplexing RPC client over any string-frame duplex ([`crate::memory_client`] or
/// [`connect_ws`]). Cheap to clone-by-Arc internally; use one per connection.
pub struct RpcClient {
    out: Outbound,
    shared: Arc<Shared>,
    next_id: AtomicU64,
    reader: tokio::task::JoinHandle<()>,
}

/// Owned stream receiver whose drop immediately cancels the server task.
pub struct RpcSubscription {
    retired: tokio_util::sync::CancellationToken,
    id: u64,
    items: mpsc::Receiver<serde_json::Value>,
    out: mpsc::Sender<String>,
    shared: Arc<Shared>,
}

/// A proxy keeps the original reply kind and wire error, including null values
/// and readiness acknowledgements. Dropping it cancels only that RPC request.
pub struct RpcForward {
    id: u64,
    readiness_ack: bool,
    ended: bool,
    frames: mpsc::Receiver<ServerFrame>,
    _cancel: PendingCancellation,
}
impl RpcForward {
    pub async fn recv(&mut self) -> Option<ServerFrame> {
        match self.frames.recv().await {
            Some(frame) => {
                if frame.err.is_some() || frame.done || (frame.ok.is_some() && !self.readiness_ack)
                {
                    self.ended = true;
                }
                Some(frame)
            }
            None if !self.ended => {
                self.ended = true;
                Some(ServerFrame { id: self.id, err: Some("Cloud connection interrupted. This action may have completed; check its state before retrying.".into()), ..Default::default() })
            }
            None => None,
        }
    }
}

impl RpcSubscription {
    pub async fn recv(&mut self) -> Option<serde_json::Value> {
        tokio::select! {biased;_=self.retired.cancelled()=>None,item=self.items.recv()=>item}
    }
}

impl Drop for RpcSubscription {
    fn drop(&mut self) {
        drop(PendingCancellation {
            id: self.id,
            out: self.out.clone(),
            shared: self.shared.clone(),
        });
    }
}

struct PendingCancellation {
    id: u64,
    out: mpsc::Sender<String>,
    shared: Arc<Shared>,
}

impl Drop for PendingCancellation {
    fn drop(&mut self) {
        if self.shared.lock().remove(&self.id).is_none() {
            return;
        }
        let Ok(frame) = serde_json::to_string(&ClientFrame {
            id: self.id,
            method: None,
            params: serde_json::Value::Null,
            cancel: true,
        }) else {
            return;
        };
        match self.out.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(frame)) => {
                let out = self.out.clone();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = out.send(frame).await;
                    });
                }
            }
        }
    }
}

impl RpcClient {
    /// Wrap an existing duplex: `out` carries client frames, `inbound` server frames.
    pub fn new(out: mpsc::Sender<String>, inbound: mpsc::Receiver<String>) -> Self {
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
        });
        let retired = tokio_util::sync::CancellationToken::new();
        Self::start(
            Outbound::Fixed(super::reconnect::Session {
                sender: out,
                retired: retired.clone(),
            }),
            Inbox::Fixed {
                input: inbound,
                retired,
            },
            shared,
        )
    }
    /// Preserve client identity across explicitly authenticated replacement sockets.
    /// Pending requests fail at the boundary; this client never replays a frame.
    pub fn reconnecting() -> (Self, super::reconnect::Control) {
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
        });
        let (input, inbound) = mpsc::channel(16);
        let state = super::reconnect::State::new(Arc::downgrade(&shared), input);
        let control = super::reconnect::Control::new(state.clone());
        (
            Self::start(
                Outbound::Reconnect(state),
                Inbox::Reconnect(inbound),
                shared,
            ),
            control,
        )
    }
    fn start(out: Outbound, mut inbound: Inbox, shared: Arc<Shared>) -> Self {
        let reader_shared = shared.clone();
        let reader_out = out.clone();
        let reader = tokio::spawn(async move {
            while let Some(payload) = inbound.recv().await {
                if payload.retired.is_cancelled() {
                    continue;
                }
                for line in payload.body.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let frame: ServerFrame = match serde_json::from_str(line) {
                        Ok(frame) => frame,
                        Err(err) => {
                            tracing::warn!(error = %err, "rpc: dropping malformed server frame");
                            continue;
                        }
                    };
                    tokio::select! {biased;_=payload.retired.cancelled()=>break,_=route_frame(&reader_shared,&reader_out,frame)=>{}}
                }
            }
            // Connection closed: fail everything still pending.
            reader_shared.interrupt(false);
        });
        Self {
            out,
            shared,
            next_id: AtomicU64::new(1),
            reader,
        }
    }

    /// Forward a canonical request through a shared client without guessing
    /// whether its reply will be unary, streaming or a versioned stream ack.
    pub async fn forward(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcForward, RpcError> {
        let session = self.out.snapshot()?;
        let out = session.sender;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (frames, receiver) = mpsc::channel(8);
        self.shared.lock().insert(
            id,
            Pending::Forward {
                frames,
                readiness_ack: method == crate::methods::WATCH_CHECKOUT_CHANGE_REQUEST,
            },
        );
        let forward = RpcForward {
            id,
            readiness_ack: method == crate::methods::WATCH_CHECKOUT_CHANGE_REQUEST,
            ended: false,
            frames: receiver,
            _cancel: PendingCancellation {
                id,
                out: out.clone(),
                shared: self.shared.clone(),
            },
        };
        Self::send(
            &out,
            ClientFrame {
                id,
                method: Some(method.into()),
                params,
                cancel: false,
            },
        )
        .await?;
        Ok(forward)
    }

    /// Unary request.
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError> {
        self.call_inner(method, params, false).await
    }

    /// A unary call whose caller may cancel execution by dropping the future.
    /// Use only for handlers designed to tolerate cancellation during execution.
    pub async fn call_scoped(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError> {
        self.call_inner(method, params, true).await
    }

    async fn call_inner(
        &self,
        method: &str,
        params: serde_json::Value,
        cancel_on_drop: bool,
    ) -> Result<serde_json::Value, RpcError> {
        let session = self.out.snapshot()?;
        let out = session.sender;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.shared.lock().insert(id, Pending::Call(tx));
        let _cancel = cancel_on_drop.then(|| PendingCancellation {
            id,
            out: out.clone(),
            shared: self.shared.clone(),
        });
        Self::send(
            &out,
            ClientFrame {
                id,
                method: Some(method.into()),
                params,
                cancel: false,
            },
        )
        .await
        .inspect_err(|_| {
            self.shared.lock().remove(&id);
        })?;
        rx.await.map_err(|_| RpcError::Closed)?
    }

    /// Typed unary request.
    pub async fn call_as<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, RpcError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| RpcError::BadParams(e.to_string()))
    }

    /// Legacy streaming request. Cancellation on receiver drop is only detected
    /// when another item arrives. Prefer `subscribe_scoped` for owned streams.
    pub async fn subscribe(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<mpsc::Receiver<serde_json::Value>, RpcError> {
        let session = self.out.snapshot()?;
        let out = session.sender;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(STREAM_QUEUE_CAP);
        self.shared.lock().insert(id, Pending::Stream(tx));
        Self::send(
            &out,
            ClientFrame {
                id,
                method: Some(method.into()),
                params,
                cancel: false,
            },
        )
        .await
        .inspect_err(|_| {
            self.shared.lock().remove(&id);
        })?;
        Ok(rx)
    }

    /// A drop-cancelled stream without waiting for its first item. Unlike a
    /// checked subscription this also works for legitimately silent streams.
    pub async fn subscribe_scoped(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcSubscription, RpcError> {
        let session = self.out.snapshot()?;
        let out = session.sender;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(STREAM_QUEUE_CAP);
        self.shared.lock().insert(id, Pending::Stream(tx));
        // Own cancellation before the send, including cancellation while the
        // outbound channel is backpressured.
        let subscription = RpcSubscription {
            retired: session.retired,
            id,
            items: rx,
            out: out.clone(),
            shared: self.shared.clone(),
        };
        Self::send(
            &out,
            ClientFrame {
                id,
                method: Some(method.into()),
                params,
                cancel: false,
            },
        )
        .await?;
        Ok(subscription)
    }

    /// Streaming request with a server acknowledgement before returning.
    ///
    /// Use this for optional/versioned stream methods: an older server's
    /// `unknown method` is returned distinctly instead of looking like an empty stream.
    pub async fn subscribe_checked(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcSubscription, RpcError> {
        let session = self.out.snapshot()?;
        let out = session.sender;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // PNG preview frames are disposable. Keep a stalled viewer from retaining
        // hundreds of images or blocking unrelated replies on this connection.
        let drop_when_full = method == crate::methods::SIMULATOR_FRAMES;
        let (items_tx, items_rx) = mpsc::channel(if drop_when_full { 2 } else { STREAM_QUEUE_CAP });
        let (ready_tx, ready_rx) = oneshot::channel();
        self.shared.lock().insert(
            id,
            Pending::CheckedStream {
                items: items_tx,
                ready: Some(ready_tx),
                drop_when_full,
            },
        );
        let subscription = RpcSubscription {
            retired: session.retired,
            id,
            items: items_rx,
            out: out.clone(),
            shared: self.shared.clone(),
        };
        Self::send(
            &out,
            ClientFrame {
                id,
                method: Some(method.into()),
                params,
                cancel: false,
            },
        )
        .await
        .inspect_err(|_| {
            self.shared.lock().remove(&id);
        })?;
        ready_rx.await.map_err(|_| RpcError::Closed)??;
        Ok(subscription)
    }

    async fn send(out: &mpsc::Sender<String>, frame: ClientFrame) -> Result<(), RpcError> {
        let json = serde_json::to_string(&frame)
            .map_err(|e| RpcError::Transport(format!("serialize frame: {e}")))?;
        out.send(json).await.map_err(|_| RpcError::Closed)
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        match &self.out {
            Outbound::Reconnect(state) => state.close(),
            Outbound::Fixed(session) => session.retired.cancel(),
        }
        self.reader.abort();
    }
}

async fn route_frame(shared: &Arc<Shared>, out: &Outbound, frame: ServerFrame) {
    let id = frame.id;
    let forward = {
        let mut pending = shared.lock();
        match pending.get(&id) {
            Some(Pending::Forward {
                frames,
                readiness_ack,
            }) => {
                let sender = frames.clone();
                if frame.err.is_some() || frame.done || (frame.ok.is_some() && !readiness_ack) {
                    pending.remove(&id);
                }
                Some(sender)
            }
            _ => None,
        }
    };
    if let Some(sender) = forward {
        if sender.send(frame).await.is_err() {
            if let Ok(session) = out.snapshot() {
                drop(PendingCancellation {
                    id,
                    out: session.sender,
                    shared: shared.clone(),
                });
            }
        }
        return;
    }
    if let Some(err) = frame.err {
        let error = wire_error(err);
        match shared.lock().remove(&id) {
            Some(Pending::Call(tx)) => {
                let _ = tx.send(Err(error));
            }
            Some(Pending::CheckedStream {
                ready: Some(ready), ..
            }) => {
                let _ = ready.send(Err(error));
            }
            Some(Pending::Stream(_))
            | Some(Pending::Forward { .. })
            | Some(Pending::CheckedStream { ready: None, .. })
            | None => {
                // Stream errored: the sender drop closes the receiver.
                tracing::debug!(id, %error, "rpc: stream ended with error");
            }
        }
        return;
    }
    if let Some(value) = frame.ok {
        let mut pending = shared.lock();
        if matches!(pending.get(&id), Some(Pending::Call(_))) {
            if let Some(Pending::Call(tx)) = pending.remove(&id) {
                let _ = tx.send(Ok(value));
            }
        } else if let Some(Pending::CheckedStream { ready, .. }) = pending.get_mut(&id)
            && let Some(ready) = ready.take()
        {
            let _ = ready.send(Ok(()));
        }
        return;
    }
    if let Some(item) = frame.item {
        let ready = match shared.lock().get_mut(&id) {
            Some(Pending::CheckedStream { ready, .. }) => ready.take(),
            _ => None,
        };
        if let Some(ready) = ready {
            let _ = ready.send(Ok(()));
        }
        // Clone the sender out of the lock: the bounded send must await
        // (backpressure) without holding `shared`.
        let tx = match shared.lock().get(&id) {
            Some(Pending::Stream(tx)) => Some((tx.clone(), false)),
            Some(Pending::CheckedStream {
                items,
                drop_when_full,
                ..
            }) => Some((items.clone(), *drop_when_full)),
            _ => None,
        };
        let dead = match tx {
            Some((tx, true)) => {
                matches!(tx.try_send(item), Err(mpsc::error::TrySendError::Closed(_)))
            }
            Some((tx, false)) => tx.send(item).await.is_err(),
            None => false,
        };
        if dead {
            // Receiver was dropped — cancel server-side and forget the stream.
            shared.lock().remove(&id);
            if let Ok(json) = serde_json::to_string(&ClientFrame {
                id,
                method: None,
                params: serde_json::Value::Null,
                cancel: true,
            }) {
                if let Ok(out) = out.snapshot() {
                    let _ = out.sender.send(json).await;
                }
            }
        }
        return;
    }
    if frame.done
        && let Some(Pending::CheckedStream {
            ready: Some(ready), ..
        }) = shared.lock().remove(&id)
    {
        let _ = ready.send(Ok(()));
    }
}

fn wire_error(error: String) -> RpcError {
    if let Some(method) = error.strip_prefix("unknown method: ") {
        RpcError::UnknownMethod(method.to_owned())
    } else if let Some(code) = error.strip_prefix("capability error: ") {
        RpcError::Capability(code.to_owned())
    } else {
        RpcError::Failed(error)
    }
}

/// How long a dial may take before we give up.
///
/// This is localhost: a real engine answers in milliseconds. Without a bound,
/// *any* other process holding the port accepts the TCP connection and then
/// never completes the WebSocket handshake, and the caller waits forever — a
/// stranger on port 27654 would hang the app at boot rather than degrade it.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Dial a WebSocket RPC server (`ws://127.0.0.1:{ipc_port}`).
pub async fn connect_ws(url: &str) -> Result<RpcClient, RpcError> {
    let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| RpcError::Transport(format!("timed out dialing {url}")))?
        .map_err(|e| RpcError::Transport(e.to_string()))?;
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(256);
    let (in_tx, in_rx) = mpsc::channel::<String>(256);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                frame = out_rx.recv() => match frame {
                    Some(text) => {
                        if sink.send(WsMessage::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        let _ = sink.send(WsMessage::Close(None)).await;
                        break;
                    }
                },
                message = stream.next() => match message {
                    Some(Ok(WsMessage::Text(text))) => {
                        if in_tx.send(text).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
            }
        }
    });
    Ok(RpcClient::new(out_tx, in_rx))
}

#[cfg(test)]
#[path = "client/forward_tests.rs"]
mod forward_tests;

#[cfg(test)]
mod simulator_queue_tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn simulator_frames_are_bounded_without_blocking_other_replies() {
        let (out, mut requests) = mpsc::channel(16);
        let (replies, inbound) = mpsc::channel(16);
        let client = Arc::new(RpcClient::new(out, inbound));
        let subscribing = client.clone();
        let task = tokio::spawn(async move {
            subscribing
                .subscribe_checked(crate::methods::SIMULATOR_FRAMES, json!({}))
                .await
                .unwrap()
        });
        let request: ClientFrame = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
        let frame = |n| {
            serde_json::to_string(&ServerFrame {
                id: request.id,
                item: Some(json!({"frame":n})),
                ..Default::default()
            })
            .unwrap()
        };
        replies.send(frame(0)).await.unwrap();
        let subscription = task.await.unwrap();
        for n in 1..8 {
            replies.send(frame(n)).await.unwrap();
        }
        let calling = client.clone();
        let call = tokio::spawn(async move { calling.call("Ping", json!({})).await });
        let ping: ClientFrame = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
        replies
            .send(
                serde_json::to_string(&ServerFrame {
                    id: ping.id,
                    ok: Some(json!({"pong":true})),
                    ..Default::default()
                })
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), call)
                .await
                .unwrap()
                .unwrap()
                .unwrap()["pong"],
            true
        );
        assert_eq!(subscription.items.len(), 2);
        drop(subscription);
        let cancel: ClientFrame = serde_json::from_str(&requests.recv().await.unwrap()).unwrap();
        assert!(cancel.cancel);
        assert_eq!(cancel.id, request.id);
    }
}
