//! Pair-authorized binary RPC. This endpoint never accepts plaintext RPC frames.
use crate::BrowserServer;
use axum::{
    extract::{Path, State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::{Sink, SinkExt, Stream, StreamExt};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use zuse_remote::{
    MAX_RECORD_BYTES, PairId, SecureChannel,
    authority::{Authority, Credentials},
};
use zuse_rpc::{RpcError, RpcReply, RpcService};

pub(super) async fn upgrade(
    State(server): State<BrowserServer>,
    Path(pair_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some((authority, device)) = server.pairings.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(id) = PairId::parse(&pair_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(credentials) = authority.credentials(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(permit) = server.paired_slots.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let connections = server.connections.clone();
    ws.max_message_size(MAX_RECORD_BYTES)
        .on_upgrade(move |socket| {
            connections.track_future(async move {
                let _permit = permit;
                session(server, authority, device, Arc::new(credentials), socket).await;
            })
        })
}
pub(super) struct Authorized {
    pub(super) inner: Arc<dyn RpcService>,
    pub(super) credentials: Arc<Credentials>,
    pub(super) device: String,
}
#[async_trait::async_trait]
impl RpcService for Authorized {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if !self.credentials.is_live() {
            return Err(RpcError::Failed("Pairing authorization was revoked".into()));
        }
        if params
            .get("targetDeviceId")
            .filter(|v| !v.is_null())
            .is_some_and(|target| target.as_str() != Some(self.device.as_str()))
        {
            return Err(RpcError::BadParams(
                "Connect directly to the paired execution host".into(),
            ));
        }
        self.inner.handle(method, params).await
    }
}
async fn binary<R, W, E>(reader: &mut R, writer: &mut W) -> Result<Vec<u8>, ()>
where
    R: Stream<Item = Result<Message, E>> + Unpin,
    W: Sink<Message> + Unpin,
{
    loop {
        match reader.next().await {
            Some(Ok(Message::Binary(bytes))) => return Ok(bytes.to_vec()),
            Some(Ok(Message::Ping(bytes))) => send(writer, Message::Pong(bytes)).await?,
            Some(Ok(Message::Pong(_))) => {}
            _ => return Err(()),
        }
    }
}
async fn message<R, W, E>(
    channel: &mut SecureChannel,
    reader: &mut R,
    writer: &mut W,
) -> Result<Vec<u8>, ()>
where
    R: Stream<Item = Result<Message, E>> + Unpin,
    W: Sink<Message> + Unpin,
{
    loop {
        let record = binary(reader, writer).await?;
        if let Some(bytes) = channel.open(&record).map_err(|_| ())? {
            return Ok(bytes);
        }
    }
}
fn valid_first(frame: &str) -> bool {
    let mut request = false;
    for line in frame.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(frame) = serde_json::from_str::<zuse_rpc::ClientFrame>(line) else {
            return false;
        };
        if frame
            .method
            .as_ref()
            .is_some_and(|method| !method.is_empty())
            && !frame.cancel
        {
            request = true;
        } else if !frame.cancel {
            return false;
        }
    }
    request
}
pub(crate) async fn session<S, E>(
    server: BrowserServer,
    authority: Authority,
    device: String,
    credentials: Arc<Credentials>,
    socket: S,
) where
    S: Stream<Item = Result<Message, E>> + Sink<Message> + Unpin,
{
    if server.shutdown.is_cancelled() || !credentials.is_live() {
        return;
    }
    let (mut writer, mut reader) = socket.split();
    let authenticate = async {
        let first = tokio::time::timeout(Duration::from_secs(10), binary(&mut reader, &mut writer))
            .await
            .map_err(|_| ())??;
        let (mut channel, reply) =
            zuse_remote::accept(credentials.id(), credentials.secret(), &first).map_err(|_| ())?;
        send(&mut writer, Message::Binary(reply.into())).await?;
        let first = tokio::time::timeout(
            Duration::from_secs(30),
            message(&mut channel, &mut reader, &mut writer),
        )
        .await
        .map_err(|_| ())??;
        let control = zuse_remote::heartbeat::response(&first).map_err(|_| ())?;
        let first = if control.is_some() {
            None
        } else {
            let first = String::from_utf8(first).map_err(|_| ())?;
            if !valid_first(&first) {
                return Err(());
            }
            Some(first)
        };
        authority.confirm(&credentials).map_err(|_| ())?;
        Ok::<_, ()>((channel, first, control))
    };
    let authenticated = tokio::select! { biased;
        _ = server.shutdown.cancelled() => return,
        _ = credentials.revoked() => return,
        result = authenticate => result,
    };
    let Ok((mut channel, first, control)) = authenticated else {
        return;
    };
    let heartbeat_peer = control.is_some();
    if let Some(control) = control
        && encrypted(&mut channel, &mut writer, &control)
            .await
            .is_err()
    {
        return;
    }
    let first = match first {
        Some(first) => first,
        None => {
            let opened = tokio::select! { biased;
                _ = server.shutdown.cancelled() => Err(()),
                _ = credentials.revoked() => Err(()),
                first = first_application(&mut channel, &mut reader, &mut writer, &credentials) => first,
            };
            let Ok(first) = opened else {
                channel.close();
                return;
            };
            first
        }
    };
    if let Some(request) = crate::paired_session::attach_request(&first) {
        crate::paired_session::run(
            server,
            device,
            credentials,
            channel,
            reader,
            writer,
            request,
        )
        .await;
        return;
    }
    let (out_tx, mut out_rx) =
        mpsc::channel::<zuse_rpc::OutputFrame>(crate::SOCKET_OUTPUT_CAPACITY);
    let (in_tx, in_rx) = mpsc::channel::<String>(16);
    let service = Arc::new(Authorized {
        inner: server.service,
        credentials: credentials.clone(),
        device,
    });
    let dispatch = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
        zuse_rpc::serve_connection_bounded(service, out_tx, in_rx, crate::SOCKET_OUTPUT_BYTES),
    ));
    {
        let exchange = async {
            let mut heartbeat_peer = heartbeat_peer;
            let mut last_encrypted = tokio::time::Instant::now();
            in_tx.send(first).await.map_err(|_| ())?;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(last_encrypted + Duration::from_secs(90)), if heartbeat_peer => return Err(()),
                    data = out_rx.recv() => {
                        let Some(data) = data else { return Ok::<_, ()>(()); };
                        encrypted(&mut channel, &mut writer, data.as_bytes()).await?;
                    },
                    frame = reader.next() => match frame {
                        Some(Ok(Message::Binary(record))) => {
                            if let Some(bytes) = channel.open(&record).map_err(|_| ())? {
                                last_encrypted = tokio::time::Instant::now();
                                if let Some(reply) = zuse_remote::heartbeat::response(&bytes).map_err(|_|())? {
                                    if !credentials.is_live() { return Err(()); }
                                    heartbeat_peer = true;
                                    encrypted(&mut channel, &mut writer, &reply).await?;
                                } else {
                                    let payload = String::from_utf8(bytes).map_err(|_| ())?;
                                    tokio::time::timeout(Duration::from_secs(30),in_tx.send(payload)).await.map_err(|_|())?.map_err(|_| ())?;
                                }
                            }
                        },
                        Some(Ok(Message::Ping(data))) => send(&mut writer, Message::Pong(data)).await?,
                        Some(Ok(Message::Pong(_))) => {},
                        _ => return Ok(()),
                    }
                }
            }
        };
        tokio::select! { biased;
            _ = server.shutdown.cancelled() => {},
            _ = credentials.revoked() => {},
            _ = exchange => {},
        }
    }
    channel.close();
    drop(in_tx);
    let _ = dispatch.await;
}

/// Answer heartbeats until the first application message shows which protocol
/// this client speaks. Probes close here without ever opening a session.
async fn first_application<R, W, E>(
    channel: &mut SecureChannel,
    reader: &mut R,
    writer: &mut W,
    credentials: &Credentials,
) -> Result<String, ()>
where
    R: Stream<Item = Result<Message, E>> + Unpin,
    W: Sink<Message> + Unpin,
{
    loop {
        let bytes = tokio::time::timeout(Duration::from_secs(90), message(channel, reader, writer))
            .await
            .map_err(|_| ())??;
        match zuse_remote::heartbeat::response(&bytes).map_err(|_| ())? {
            Some(reply) => {
                if !credentials.is_live() {
                    return Err(());
                }
                encrypted(channel, writer, &reply).await?;
            }
            None => return String::from_utf8(bytes).map_err(|_| ()),
        }
    }
}

pub(super) async fn send<W: Sink<Message> + Unpin>(
    writer: &mut W,
    message: Message,
) -> Result<(), ()> {
    tokio::time::timeout(Duration::from_secs(30), writer.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

pub(super) async fn encrypted<W: Sink<Message> + Unpin>(
    channel: &mut SecureChannel,
    writer: &mut W,
    payload: &[u8],
) -> Result<(), ()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        for record in channel.seal(payload).map_err(|_| ())? {
            send(writer, Message::Binary(record.into())).await?;
        }
        Ok(())
    })
    .await
    .map_err(|_| ())?
}
