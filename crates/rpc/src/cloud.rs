//! Explicit Rust RPC over the cloud workspace's opaque multiplexed gateway.
//! Socket lifetimes own RPC subscriptions, never the engine or accepted agent work.
use crate::{RpcClient, RpcError, RpcService, reconnect::Channel};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use zuse_protocol::cloud_gateway::{Direction, Frame, MAX_FRAME, Payload, RUST_PROTOCOL, RUST_RPC};

/// Carries secrets; intentionally has no Debug implementation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub workspace_id: String,
    pub ws_url: String,
    pub protocol: String,
    pub rpc_protocol: String,
    pub credential: String,
    pub generation: i64,
    pub gateway_epoch: i64,
    pub expires_at: i64,
}
fn rejected() -> RpcError {
    RpcError::Transport("Cloud workspace connection authorization failed".into())
}
fn millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
fn request(
    grant: &Grant,
    workspace: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, RpcError> {
    let url: tokio_tungstenite::tungstenite::http::Uri =
        grant.ws_url.parse().map_err(|_| rejected())?;
    let host = url.host().ok_or_else(rejected)?;
    let loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if grant.workspace_id != workspace
        || grant.generation < 1
        || grant.gateway_epoch < 1
        || grant.rpc_protocol != RUST_RPC
        || grant.credential.is_empty()
        || grant.credential.len() > 16384
        || grant.expires_at <= millis()
        || grant.protocol != RUST_PROTOCOL
        || !(url.scheme_str() == Some("wss") || (loopback && url.scheme_str() == Some("ws")))
        || url.authority().is_none_or(|a| a.as_str().contains('@'))
        || url.query().is_some()
        || url.path() != format!("/v1/cloud/workspaces/{workspace}/gateway")
    {
        return Err(rejected());
    }
    let mut request = grant
        .ws_url
        .as_str()
        .into_client_request()
        .map_err(|_| rejected())?;
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{}, {}", grant.protocol, grant.credential)
            .parse()
            .map_err(|_| rejected())?,
    );
    Ok(request)
}
async fn dial(
    grant: &Grant,
    workspace: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    RpcError,
> {
    let request = request(grant, workspace)?;
    let config = WebSocketConfig {
        max_message_size: Some(MAX_FRAME),
        max_frame_size: Some(MAX_FRAME),
        ..Default::default()
    };
    let (socket, response) = tokio::time::timeout(
        Duration::from_secs(10),
        connect_async_with_config(request, Some(config), false),
    )
    .await
    .map_err(|_| RpcError::Transport("Cloud gateway connection timed out".into()))?
    .map_err(|_| rejected())?;
    if response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        != Some(grant.protocol.as_str())
    {
        return Err(rejected());
    }
    Ok(socket)
}
/// Opens one verified channel. A new grant/connection is required after failure;
/// canonical RPC frames are never automatically resent on another socket.
pub async fn connect_channel(
    grant: Grant,
    workspace: &str,
    cancel: CancellationToken,
) -> Result<Channel, RpcError> {
    let mut socket = tokio::select! {biased; _=cancel.cancelled()=>return Err(RpcError::Closed),socket=dial(&grant,workspace)=>socket?};
    let ready = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let value: serde_json::Value =
                        serde_json::from_str(&text).map_err(|_| rejected())?;
                    if value["_zuseGateway"] != "availability" {
                        return Err(rejected());
                    }
                    match value["state"].as_str() {
                        Some("available") if value["reset"] != true => return Ok(()),
                        Some("pending") => {}
                        _ => return Err(RpcError::Closed),
                    }
                }
                Some(Ok(Message::Ping(bytes))) => socket
                    .send(Message::Pong(bytes))
                    .await
                    .map_err(|_| RpcError::Closed)?,
                _ => return Err(RpcError::Closed),
            }
        }
    };
    tokio::select! {biased; _=cancel.cancelled()=>return Err(RpcError::Closed),ready=tokio::time::timeout(Duration::from_secs(35),ready)=>ready.map_err(|_|RpcError::Transport("Cloud runtime did not become available".into()))??}
    let (outgoing, mut out) = mpsc::channel::<String>(8);
    let (input, incoming) = mpsc::channel::<String>(8);
    let closed = cancel.child_token();
    let stop = closed.clone();
    tokio::spawn(async move {
        let _closed = stop.clone().drop_guard();
        loop {
            tokio::select! {biased;
                _=stop.cancelled()=>break,
                next=socket.next()=> match next {
                    Some(Ok(Message::Text(text)))=> {
                        if text.len()<=512 && serde_json::from_str::<serde_json::Value>(&text).ok().is_some_and(|v|v["_zuseGateway"]=="availability") {break;}
                        if !matches!(tokio::time::timeout(Duration::from_secs(2),input.send(text.to_string())).await,Ok(Ok(()))) {break;}
                    }
                    Some(Ok(Message::Ping(bytes)))=>{if !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Pong(bytes))).await,Ok(Ok(()))) {break;}}
                    Some(Ok(Message::Pong(_)))=>{},_=>break,
                },
                next=out.recv()=> {
                    let Some(next)=next else {break;};
                    if next.len()>zuse_protocol::cloud_gateway::MAX_PAYLOAD || !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Text(next))).await,Ok(Ok(()))) {break;}
                }
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
    });
    Ok(Channel {
        outgoing,
        incoming,
        closed,
    })
}
pub struct Connection {
    pub client: RpcClient,
    pub closed: CancellationToken,
}
pub async fn connect(
    grant: Grant,
    workspace: &str,
    cancel: CancellationToken,
) -> Result<Connection, RpcError> {
    let channel = connect_channel(grant, workspace, cancel).await?;
    Ok(Connection {
        client: RpcClient::new(channel.outgoing, channel.incoming),
        closed: channel.closed,
    })
}
struct Peer {
    input: mpsc::Sender<String>,
    _task: AbortOnDropHandle<()>,
}
#[derive(Deserialize)]
#[serde(tag = "type")]
enum Control {
    #[serde(rename = "client.open", rename_all = "camelCase")]
    Open {
        connection_id: String,
        actor_id: String,
        permission: String,
    },
    #[serde(rename = "client.close", rename_all = "camelCase")]
    Close { connection_id: String },
    #[serde(rename = "runtime.command")]
    Command,
}
/// Runs the already assembled engine on one authenticated gateway attachment.
/// Returning tears down transport subscriptions; the caller retains engine ownership.
pub async fn serve_guest(
    service: Arc<dyn RpcService>,
    grant: Grant,
    workspace: &str,
    owner: &str,
    cancel: CancellationToken,
) -> Result<(), RpcError> {
    if owner.starts_with("organization:") {
        return Err(RpcError::Capability(
            "cloud.organization_access_unavailable".into(),
        ));
    }
    let mut socket = tokio::select! {biased; _=cancel.cancelled()=>return Ok(()),socket=dial(&grant,workspace)=>socket?};
    let mut peers = HashMap::<String, Peer>::new();
    let (output, mut outgoing) = mpsc::channel::<(String, String)>(16);
    loop {
        tokio::select! {biased;
            _=cancel.cancelled()=>break,
            incoming=socket.next()=>match incoming {
                Some(Ok(Message::Text(text)))=> {
                    let message:Control=serde_json::from_str(&text).map_err(|_|RpcError::Transport("Invalid cloud gateway control message".into()))?;
                    match message {
                        Control::Open {connection_id,actor_id,permission}=> {
                            if connection_id.is_empty() || connection_id.len()>512 || actor_id!=owner || permission!="edit" || peers.len()>=64 {return Err(rejected());}
                            let (input,receiver)=mpsc::channel(8);
                            let (sender,mut replies)=mpsc::channel(8);
                            let service=service.clone();let output=output.clone();let id=connection_id.clone();
                            let task=tokio::spawn(async move {
                                tokio::select! {
                                    _=crate::serve_connection(service,sender,receiver)=>{},
                                    _=async {
                                        while let Some(reply)=replies.recv().await {
                                            if output.send((id.clone(),reply)).await.is_err() {break;}
                                        }
                                    }=>{},
                                }
                            });
                            peers.insert(connection_id,Peer {input,_task:AbortOnDropHandle::new(task)});
                        }
                        Control::Close {connection_id}=>{peers.remove(&connection_id);}
                        Control::Command=>return Err(RpcError::Capability("cloud.mailbox_unavailable".into())),
                    }
                }
                Some(Ok(Message::Binary(bytes)))=> {
                    let frame=Frame::decode(&bytes).ok_or_else(rejected)?;
                    if frame.direction!=Direction::Client {return Err(rejected());}
                    if let Some(peer)=peers.get(&frame.connection_id) {
                        let Payload::Text(text)=frame.payload else {return Err(RpcError::BadParams("Rust cloud RPC requires text frames".into()));};
                        if !matches!(tokio::time::timeout(Duration::from_secs(2),peer.input.send(text)).await,Ok(Ok(()))) {return Err(RpcError::Transport("Cloud client exceeded runtime capacity".into()));}
                    }
                }
                Some(Ok(Message::Ping(bytes)))=> {tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Pong(bytes))).await.map_err(|_|RpcError::Closed)?.map_err(|_|RpcError::Closed)?;}
                Some(Ok(Message::Pong(_)))=>{},
                Some(Ok(Message::Close(Some(frame)))) if u16::from(frame.code)==4101=>return Err(RpcError::Capability("cloud.runtime_fenced".into())),
                _=>return Err(RpcError::Closed),
            },
            reply=outgoing.recv()=> {
                let Some((id,payload))=reply else {return Err(RpcError::Closed);};
                if !peers.contains_key(&id) {continue;}
                let frame=Frame {direction:Direction::Runtime,connection_id:id,payload:Payload::Text(payload)}.encode().ok_or_else(rejected)?;
                tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Binary(frame))).await.map_err(|_|RpcError::Closed)?.map_err(|_|RpcError::Closed)?;
            }
        }
    }
    peers.clear();
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
    Ok(())
}
