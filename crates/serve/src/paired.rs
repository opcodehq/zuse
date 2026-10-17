//! Pair-authorized binary RPC. This endpoint never accepts plaintext RPC frames.
use crate::BrowserServer;
use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
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
struct Authorized {
    inner: Arc<dyn RpcService>,
    credentials: Arc<Credentials>,
    device: String,
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
async fn binary(
    reader: &mut SplitStream<WebSocket>,
    writer: &mut SplitSink<WebSocket, Message>,
) -> Result<Vec<u8>, ()> {
    loop {
        match reader.next().await {
            Some(Ok(Message::Binary(bytes))) => return Ok(bytes.to_vec()),
            Some(Ok(Message::Ping(bytes))) => {
                writer.send(Message::Pong(bytes)).await.map_err(|_| ())?
            }
            Some(Ok(Message::Pong(_))) => {}
            _ => return Err(()),
        }
    }
}
async fn message(
    channel: &mut SecureChannel,
    reader: &mut SplitStream<WebSocket>,
    writer: &mut SplitSink<WebSocket, Message>,
) -> Result<String, ()> {
    loop {
        let record = binary(reader, writer).await?;
        if let Some(bytes) = channel.open(&record).map_err(|_| ())? {
            return String::from_utf8(bytes).map_err(|_| ());
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
async fn session(
    server: BrowserServer,
    authority: Authority,
    device: String,
    credentials: Arc<Credentials>,
    socket: WebSocket,
) {
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
        writer
            .send(Message::Binary(reply.into()))
            .await
            .map_err(|_| ())?;
        let first = tokio::time::timeout(
            Duration::from_secs(30),
            message(&mut channel, &mut reader, &mut writer),
        )
        .await
        .map_err(|_| ())??;
        if !valid_first(&first) {
            return Err(());
        }
        authority.confirm(&credentials).map_err(|_| ())?;
        Ok::<_, ()>((channel, first))
    };
    let authenticated = tokio::select! { biased;
        _ = server.shutdown.cancelled() => return,
        _ = credentials.revoked() => return,
        result = authenticate => result,
    };
    let Ok((mut channel, first)) = authenticated else {
        return;
    };
    let (out_tx, mut out_rx) = mpsc::channel::<String>(16);
    let (in_tx, in_rx) = mpsc::channel::<String>(16);
    let service = Arc::new(Authorized {
        inner: server.service,
        credentials: credentials.clone(),
        device,
    });
    let dispatch = tokio::spawn(zuse_rpc::serve_connection(service, out_tx, in_rx));
    {
        let exchange = async {
            in_tx.send(first).await.map_err(|_| ())?;
            loop {
                tokio::select! {
                    data = out_rx.recv() => {
                        let Some(data) = data else { return Ok::<_, ()>(()); };
                        for record in channel.seal(data.as_bytes()).map_err(|_| ())? {
                            writer.send(Message::Binary(record.into())).await.map_err(|_| ())?;
                        }
                    },
                    frame = reader.next() => match frame {
                        Some(Ok(Message::Binary(record))) => {
                            if let Some(bytes) = channel.open(&record).map_err(|_| ())? {
                                let payload = String::from_utf8(bytes).map_err(|_| ())?;
                                in_tx.send(payload).await.map_err(|_| ())?;
                            }
                        },
                        Some(Ok(Message::Ping(data))) => writer.send(Message::Pong(data)).await.map_err(|_| ())?,
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
