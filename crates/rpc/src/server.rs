//! Server side: dispatch loop over string frames + the WebSocket acceptor.

use std::collections::HashMap;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::StatusCode;

use crate::{ClientFrame, RpcError, RpcReply, RpcService, RpcStreamItem, ServerFrame};

// Bound initial transcript materialization per connection, separately from
// long-lived watches. A queued request must wait before opening its document.
const HISTORY_OPEN_CONCURRENCY: usize = 5;
// Full-history frames can each contain megabytes; hundreds of queued frames
// defeat admission control even after their request tasks finish sending.
const SOCKET_OUTPUT_CAPACITY: usize = 8;

// Dropping an AbortHandle does not cancel its task. Connection teardown must
// own cancellation even when its dispatcher future is itself aborted.
#[derive(Default)]
struct RequestTasks(HashMap<u64, tokio::task::AbortHandle>);
impl Drop for RequestTasks {
    fn drop(&mut self) {
        for task in self.0.values() {
            task.abort();
        }
    }
}

/// Serve one connection: read client frames from `inbound`, write server frames to `out`.
/// Returns when `inbound` closes; all in-flight request tasks are aborted on exit.
pub async fn serve_connection(
    service: Arc<dyn RpcService>,
    out: mpsc::Sender<String>,
    mut inbound: mpsc::Receiver<String>,
) {
    let mut running = RequestTasks::default();
    let history_openings = Arc::new(Semaphore::new(HISTORY_OPEN_CONCURRENCY));
    while let Some(payload) = inbound.recv().await {
        // ndjson: a transport may batch several frames per message.
        for line in payload.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let frame: ClientFrame = match serde_json::from_str(line) {
                Ok(frame) => frame,
                Err(err) => {
                    tracing::warn!(error = %err, "rpc: dropping malformed client frame");
                    continue;
                }
            };
            running.0.retain(|_, task| !task.is_finished());
            if frame.cancel {
                if let Some(task) = running.0.remove(&frame.id) {
                    task.abort();
                }
                continue;
            }
            let Some(method) = frame.method else {
                tracing::warn!(id = frame.id, "rpc: frame has neither method nor cancel");
                continue;
            };
            let task = tokio::spawn(handle_request(
                service.clone(),
                out.clone(),
                frame.id,
                method,
                frame.params,
                history_openings.clone(),
            ));
            if let Some(previous) = running.0.insert(frame.id, task.abort_handle()) {
                previous.abort();
            }
        }
    }
}

async fn handle_request(
    service: Arc<dyn RpcService>,
    out: mpsc::Sender<String>,
    id: u64,
    method: String,
    params: serde_json::Value,
    history_openings: Arc<Semaphore>,
) {
    let mut opening = if method == crate::methods::WATCH_DOC_MESSAGES {
        match history_openings.acquire_owned().await {
            Ok(permit) => Some(permit),
            Err(_) => return,
        }
    } else {
        None
    };
    let send = |frame: ServerFrame| {
        let out = out.clone();
        async move {
            let permit = out.reserve().await.map_err(|_| RpcError::Closed)?;
            match serde_json::to_string(&frame) {
                Ok(json) => {
                    permit.send(json);
                    Ok(())
                }
                Err(err) => {
                    tracing::error!(error = %err, "rpc: failed to serialize server frame");
                    Err(RpcError::Closed)
                }
            }
        }
    };
    let mut stream = match service.handle(&method, params).await {
        Ok(RpcReply::Value(value)) => {
            let _ = send(ServerFrame {
                id,
                ok: Some(value),
                ..Default::default()
            })
            .await;
            return;
        }
        Ok(RpcReply::Stream(stream)) => stream
            .map(|value| {
                let pending = value.get("historyPending").and_then(|v| v.as_bool()) == Some(true);
                let item = RpcStreamItem::new(value);
                if pending { item.initializing() } else { item }
            })
            .boxed(),
        Ok(RpcReply::TypedStream(stream)) => stream,
        Err(err) => {
            let _ = send(ServerFrame {
                id,
                err: Some(err.to_string()),
                ..Default::default()
            })
            .await;
            return;
        }
    };
    if method == crate::methods::WATCH_CHECKOUT_CHANGE_REQUEST
        && send(ServerFrame {
            id,
            ok: Some(serde_json::json!({"stream": true})),
            ..Default::default()
        })
        .await
        .is_err()
    {
        return;
    }
    while let Some(item) = stream.next().await {
        let pending = item.initialization_pending;
        // Poll first: idle subscriptions must never hold output capacity.
        let Ok(permit) = out.reserve().await else {
            return;
        };
        let json = match item.encode(id) {
            Ok(json) => json,
            Err(err) => {
                drop(permit);
                tracing::error!(error = %err, "rpc: failed to serialize stream item");
                let _ = send(ServerFrame {
                    id,
                    err: Some(format!("serialize response: {err}")),
                    ..Default::default()
                })
                .await;
                return;
            }
        };
        permit.send(json);
        // An opening preview retains its slot through full hydration. Idle
        // established watches must not monopolize history-opening admission.
        if !pending {
            opening.take();
        }
    }
    let _ = send(ServerFrame {
        id,
        done: true,
        ..Default::default()
    })
    .await;
}

/// Accept WebSocket connections forever, serving each with `service`.
pub async fn serve_ws_listener(listener: TcpListener, service: Arc<dyn RpcService>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tracing::debug!(%peer, "rpc: connection accepted");
                tokio::spawn(serve_ws_socket(stream, service.clone()));
            }
            Err(err) => {
                tracing::warn!(error = %err, "rpc: accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn serve_ws_socket(stream: TcpStream, service: Arc<dyn RpcService>) {
    // Native viewports dial this socket with a bare `connect_async` and send
    // no `Origin` header. A browser always attaches `Origin` to a WebSocket
    // handshake and cannot forge or suppress it from script, and WebSockets
    // are exempt from the Same-Origin Policy — so only rejecting any handshake
    // that carries `Origin` keeps a page the user happens to visit from
    // reaching this local socket. Keep this check.
    //
    // The large `Err` (ErrorResponse) is the shape tungstenite's Callback
    // trait requires; it can't be boxed away here.
    #[allow(clippy::result_large_err)]
    let reject_cross_origin = |req: &HandshakeRequest, resp: HandshakeResponse| {
        if let Some(origin) = req.headers().get("origin") {
            tracing::warn!(
                origin = %String::from_utf8_lossy(origin.as_bytes()),
                "rpc: rejecting handshake carrying an Origin header (cross-origin browser dial)"
            );
            let mut err = ErrorResponse::new(Some("origin not allowed on local IPC".to_string()));
            *err.status_mut() = StatusCode::FORBIDDEN;
            return Err(err);
        }
        Ok(resp)
    };
    let ws = match tokio_tungstenite::accept_hdr_async(stream, reject_cross_origin).await {
        Ok(ws) => ws,
        Err(err) => {
            tracing::warn!(error = %err, "rpc: websocket handshake failed");
            return;
        }
    };
    let (mut sink, mut ws_stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::channel::<String>(SOCKET_OUTPUT_CAPACITY);
    let (in_tx, in_rx) = mpsc::channel::<String>(256);

    // Pump: socket <-> string channels. Ends when either side closes.
    let pump = tokio::spawn(async move {
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
                message = ws_stream.next() => match message {
                    Some(Ok(WsMessage::Text(text))) => {
                        if in_tx.send(text).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {} // ping/pong/binary — ignored
                },
            }
        }
    });

    serve_connection(service, out_tx, in_rx).await;
    pump.abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    struct GatedHistory {
        started: mpsc::UnboundedSender<u64>,
        count: AtomicUsize,
        ready: Arc<Semaphore>,
    }

    #[async_trait::async_trait]
    impl RpcService for GatedHistory {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            if method != crate::methods::WATCH_DOC_MESSAGES {
                return Ok(RpcReply::Value(self.count.load(Ordering::SeqCst).into()));
            }
            let id = params["chat"].as_u64().unwrap();
            self.count.fetch_add(1, Ordering::SeqCst);
            self.started.send(id).unwrap();
            let ready = self.ready.clone();
            let full = futures::stream::once(async move {
                ready.acquire_owned().await.unwrap().forget();
                serde_json::json!({"chat": id, "reset": []})
            })
            .chain(futures::stream::pending())
            .boxed();
            let stream = if params["preview"].as_bool() == Some(true) {
                futures::stream::once(async {
                    serde_json::json!({"reset": [], "historyPending": true})
                })
                .chain(full)
                .boxed()
            } else {
                full
            };
            Ok(if params["typed"].as_bool() == Some(true) {
                RpcReply::TypedStream(
                    stream
                        .map(|value| {
                            let pending = value["historyPending"].as_bool() == Some(true);
                            let item = RpcStreamItem::new(value);
                            if pending { item.initializing() } else { item }
                        })
                        .boxed(),
                )
            } else {
                RpcReply::Stream(stream)
            })
        }
    }

    async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(2), future)
            .await
            .expect("RPC test made no progress")
    }

    async fn next_reply(replies: &mut mpsc::Receiver<String>) -> serde_json::Value {
        serde_json::from_str(&within(replies.recv()).await.expect("connection closed")).unwrap()
    }

    fn history_request(id: u64, preview: bool, typed: bool) -> String {
        serde_json::json!({
            "id": id,
            "method": crate::methods::WATCH_DOC_MESSAGES,
            "params": {"chat": id, "preview": preview, "typed": typed}
        })
        .to_string()
    }

    async fn bounded_history_case(typed: bool) {
        let (started, mut starts) = mpsc::unbounded_channel();
        let ready = Arc::new(Semaphore::new(0));
        let service = Arc::new(GatedHistory {
            started,
            count: AtomicUsize::new(0),
            ready: ready.clone(),
        });
        let (output, mut replies) = mpsc::channel(16);
        let (input, incoming) = mpsc::channel(16);
        let task = tokio::spawn(serve_connection(service, output, incoming));
        for id in 0..8 {
            input.send(history_request(id, false, typed)).await.unwrap();
        }
        let mut active = Vec::new();
        for _ in 0..HISTORY_OPEN_CONCURRENCY {
            active.push(within(starts.recv()).await.unwrap());
        }
        let canceled = active[0];
        input
            .send(serde_json::json!({"id":100,"method":"Probe"}).to_string())
            .await
            .unwrap();
        let probe = next_reply(&mut replies).await;
        assert_eq!(
            probe["ok"], HISTORY_OPEN_CONCURRENCY,
            "waiting history reads must not materialize more documents"
        );
        input
            .send(serde_json::json!({"id":canceled,"cancel":true}).to_string())
            .await
            .unwrap();
        let replacement = within(starts.recv()).await.unwrap();
        assert!(
            !active.contains(&replacement),
            "cancel must release admission to a waiting request"
        );
        ready.add_permits(8);
        let mut received = Vec::new();
        while received.len() < 7 {
            let reply = next_reply(&mut replies).await;
            if reply.get("item").is_some() {
                received.push(reply["id"].as_u64().unwrap());
            }
        }
        received.sort_unstable();
        assert_eq!(
            received,
            (0..8).filter(|&id| id != canceled).collect::<Vec<_>>(),
            "every uncanceled history must arrive"
        );
        drop(input);
        task.await.unwrap();
    }
    async fn opening_preview_case(typed: bool) {
        let (started, _starts) = mpsc::unbounded_channel();
        let ready = Arc::new(Semaphore::new(0));
        let service = Arc::new(GatedHistory {
            started,
            count: AtomicUsize::new(0),
            ready: ready.clone(),
        });
        let (output, mut replies) = mpsc::channel(16);
        let (input, incoming) = mpsc::channel(16);
        let task = tokio::spawn(serve_connection(service, output, incoming));
        for id in 0..8 {
            input.send(history_request(id, true, typed)).await.unwrap();
        }
        for _ in 0..HISTORY_OPEN_CONCURRENCY {
            let reply = next_reply(&mut replies).await;
            assert_eq!(reply["item"]["historyPending"], true);
        }
        input
            .send(serde_json::json!({"id":100,"method":"Probe"}).to_string())
            .await
            .unwrap();
        let probe = next_reply(&mut replies).await;
        assert_eq!(
            probe["ok"], HISTORY_OPEN_CONCURRENCY,
            "previews must not release full-history admission"
        );
        ready.add_permits(8);
        let mut full = Vec::new();
        while full.len() < 8 {
            let reply = next_reply(&mut replies).await;
            if reply["item"].get("chat").is_some() {
                full.push(reply["id"].as_u64().unwrap());
            }
        }
        full.sort_unstable();
        assert_eq!(
            full,
            (0..8).collect::<Vec<_>>(),
            "idle watches must release opening slots after their full response"
        );
        drop(input);
        task.await.unwrap();
    }
    #[tokio::test]
    async fn history_burst_is_bounded_cancelable_and_does_not_block_other_methods() {
        bounded_history_case(false).await;
    }
    #[tokio::test]
    async fn typed_history_burst_preserves_cancellation_and_unrelated_methods() {
        bounded_history_case(true).await;
    }
    #[tokio::test]
    async fn opening_previews_hold_admission_until_full_history() {
        opening_preview_case(false).await;
    }
    #[tokio::test]
    async fn typed_previews_hold_admission_until_full_history() {
        opening_preview_case(true).await;
    }

    struct CountedPayload(Arc<AtomicUsize>);
    impl serde::Serialize for CountedPayload {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            serializer.serialize_str("history")
        }
    }
    struct BufferedHistory {
        encoded: Arc<AtomicUsize>,
        polled: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl RpcService for BufferedHistory {
        async fn handle(
            &self,
            _method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            let encoded = self.encoded.clone();
            let polled = self.polled.clone();
            Ok(RpcReply::TypedStream(
                futures::stream::once(async move {
                    polled.notify_one();
                    RpcStreamItem::new(CountedPayload(encoded))
                })
                .chain(futures::stream::pending())
                .boxed(),
            ))
        }
    }

    #[tokio::test]
    async fn full_output_waits_before_encoding_and_cancellation_still_works() {
        let encoded = Arc::new(AtomicUsize::new(0));
        let polled = Arc::new(tokio::sync::Notify::new());
        let (output, mut replies) = mpsc::channel(1);
        output.send("occupied".into()).await.unwrap();
        let (input, incoming) = mpsc::channel(4);
        let task = tokio::spawn(serve_connection(
            Arc::new(BufferedHistory {
                encoded: encoded.clone(),
                polled: polled.clone(),
            }),
            output,
            incoming,
        ));
        input.send(history_request(1, false, true)).await.unwrap();
        within(polled.notified()).await;
        assert_eq!(
            encoded.load(Ordering::SeqCst),
            0,
            "a blocked socket must not allocate encoded history frames"
        );
        assert_eq!(within(replies.recv()).await.unwrap(), "occupied");
        assert_eq!(next_reply(&mut replies).await["item"], "history");
        assert_eq!(encoded.load(Ordering::SeqCst), 1);
        input
            .send(serde_json::json!({"id":1,"cancel":true}).to_string())
            .await
            .unwrap();
        drop(input);
        within(task).await.unwrap();
    }

    struct FailingPayload;
    impl serde::Serialize for FailingPayload {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional encoding failure"))
        }
    }
    struct EncodingFailureService;
    #[async_trait::async_trait]
    impl RpcService for EncodingFailureService {
        async fn handle(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            Ok(if method == crate::methods::WATCH_DOC_MESSAGES {
                RpcReply::TypedStream(
                    futures::stream::once(async { RpcStreamItem::new(FailingPayload) }).boxed(),
                )
            } else {
                RpcReply::Value(serde_json::json!("ready"))
            })
        }
    }
    #[tokio::test]
    async fn typed_encoding_failure_replies_and_releases_history_admission() {
        let (output, mut replies) = mpsc::channel(1);
        let (input, incoming) = mpsc::channel(16);
        let task = tokio::spawn(serve_connection(
            Arc::new(EncodingFailureService),
            output,
            incoming,
        ));
        for id in 0..8 {
            input.send(history_request(id, false, true)).await.unwrap();
        }
        let mut failed = Vec::new();
        while failed.len() < 8 {
            let reply = next_reply(&mut replies).await;
            assert!(
                reply["err"]
                    .as_str()
                    .unwrap()
                    .contains("intentional encoding failure")
            );
            failed.push(reply["id"].as_u64().unwrap());
        }
        failed.sort_unstable();
        assert_eq!(failed, (0..8).collect::<Vec<_>>());
        input
            .send(serde_json::json!({"id":100,"method":"Probe"}).to_string())
            .await
            .unwrap();
        assert_eq!(next_reply(&mut replies).await["ok"], "ready");
        drop(input);
        task.await.unwrap();
    }
}
