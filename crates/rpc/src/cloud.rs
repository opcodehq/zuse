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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_binding: Option<serde_json::Value>,
}
/// How often a gateway socket's owner (the guest, or a desktop channel) checks
/// it. The gateway pings every 15s and drops a peer it has not heard from in
/// 45s; both ends mirror that so neither keeps a half-open socket.
const HEARTBEAT_TICK: Duration = Duration::from_secs(5);
const PING_AFTER: Duration = Duration::from_secs(15);
const SILENCE_LIMIT: Duration = Duration::from_secs(45);
/// A tick arriving this late means the machine itself was paused (or starved):
/// the gateway has certainly closed this socket, so don't wait for silence.
const PAUSE_GAP: Duration = Duration::from_secs(20);

#[derive(Debug, PartialEq, Eq)]
enum Heartbeat {
    Idle,
    Ping,
    Reconnect,
}
/// Pure liveness bookkeeping for the guest's gateway socket.
struct Liveness {
    received: std::time::Instant,
    pinged: std::time::Instant,
    ticked: std::time::Instant,
}
impl Liveness {
    fn new(now: std::time::Instant) -> Self {
        Self {
            received: now,
            pinged: now,
            ticked: now,
        }
    }
    fn received(&mut self, now: std::time::Instant) {
        self.received = now;
    }
    fn tick(&mut self, now: std::time::Instant) -> Heartbeat {
        let gap = now.saturating_duration_since(self.ticked);
        self.ticked = now;
        if gap >= PAUSE_GAP || now.saturating_duration_since(self.received) >= SILENCE_LIMIT {
            return Heartbeat::Reconnect;
        }
        if now.saturating_duration_since(self.pinged) >= PING_AFTER {
            self.pinged = now;
            return Heartbeat::Ping;
        }
        Heartbeat::Idle
    }
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
    if let Some(binding) = &grant.storage_binding {
        let encoded = binding.to_string();
        if encoded.len() > 4096 {
            return Err(rejected());
        }
        request.headers_mut().insert(
            zuse_protocol::cloud::STARTUP_BIND_HEADER,
            encoded.parse().map_err(|_| rejected())?,
        );
    }
    Ok(request)
}
fn handshake_error(error: &tokio_tungstenite::tungstenite::Error) -> RpcError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        let status = response.status();
        if status.is_server_error() || status.as_u16() == 429 || status.as_u16() == 409 {
            return RpcError::Transport("Cloud gateway is temporarily unavailable".into());
        }
    }
    rejected()
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
    .map_err(|error| handshake_error(&error))?;
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
    let (outgoing, out) = mpsc::channel::<String>(8);
    let (input, incoming) = mpsc::channel::<String>(8);
    let closed = cancel.child_token();
    tokio::spawn(pump(socket, input, out, closed.clone()));
    Ok(Channel {
        outgoing,
        incoming,
        closed,
        engine_info: None,
    })
}
/// Carries one desktop channel until either side ends it. The gateway pings
/// every 15s, so a socket that stays silent for 45s (a sleeping laptop, a
/// network change, a dropped NAT entry) is half-open: close it so the
/// controller reconnects and every watch resubscribes, instead of leaving the
/// chat on a transcript that silently stopped updating. The same bookkeeping
/// keeps the runtime's side of the gateway honest.
async fn pump<S>(
    mut socket: tokio_tungstenite::WebSocketStream<S>,
    input: mpsc::Sender<String>,
    mut out: mpsc::Receiver<String>,
    stop: CancellationToken,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let _closed = stop.clone().drop_guard();
    // Tokio's clock is the same monotonic clock in production; tests pause it.
    let now = || tokio::time::Instant::now().into_std();
    let mut liveness = Liveness::new(now());
    let mut heartbeat = tokio::time::interval(HEARTBEAT_TICK);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        tokio::select! {biased;
            _=stop.cancelled()=>break,
            _=heartbeat.tick()=>match liveness.tick(now()) {
                Heartbeat::Idle=>{},
                Heartbeat::Ping=>{if !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Ping(Vec::new().into()))).await,Ok(Ok(()))) {break;}}
                Heartbeat::Reconnect=>break,
            },
            next=socket.next()=> {liveness.received(now());match next {
                Some(Ok(Message::Text(text)))=> {
                    if text.len()<=512 && serde_json::from_str::<serde_json::Value>(&text).ok().is_some_and(|v|v["_zuseGateway"]=="availability") {break;}
                    if !matches!(tokio::time::timeout(Duration::from_secs(2),input.send(text.to_string())).await,Ok(Ok(()))) {break;}
                }
                Some(Ok(Message::Ping(bytes)))=>{if !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Pong(bytes))).await,Ok(Ok(()))) {break;}}
                Some(Ok(Message::Pong(_)))=>{},_=>break,
            }},
            next=out.recv()=> {
                let Some(next)=next else {break;};
                if next.len()>zuse_protocol::cloud_gateway::MAX_PAYLOAD || !matches!(tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Text(next))).await,Ok(Ok(()))) {break;}
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
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
    /// `zuse_protocol::cloud_runtime_updates::POLL_NOW_METHOD`.
    #[serde(rename = "runtime.update.pollNow")]
    RuntimeUpdatePollNow,
}
/// Woken when the gateway asks this runtime to poll for a runtime update now.
/// Like `runtime.command`, the nudge carries no authority: the runtime still
/// fetches and verifies the signed directive itself.
pub fn runtime_update_poll_requests() -> &'static tokio::sync::Notify {
    static REQUESTS: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    REQUESTS.get_or_init(tokio::sync::Notify::new)
}
/// "You have a command" hints from the gateway. A hint carries no command and
/// no authority: the runtime still leases from its durable mailbox, which
/// stays the only source of truth. Hints only decide when to ask.
pub struct CommandPush {
    hints: tokio::sync::Notify,
    active: tokio::sync::watch::Sender<bool>,
}
impl Default for CommandPush {
    fn default() -> Self {
        Self {
            hints: tokio::sync::Notify::new(),
            active: tokio::sync::watch::Sender::new(false),
        }
    }
}
impl CommandPush {
    /// Ask the mailbox to lease now. Hints that arrive while it is busy
    /// coalesce into one pending wake, so none is lost.
    pub fn hint(&self) {
        self.hints.notify_one();
    }
    /// Resolves at the next hint, or at once for a hint already pending.
    pub async fn hinted(&self) {
        self.hints.notified().await
    }
    /// True while the current gateway connection has proven it sends hints.
    /// Older servers never send them, so the runtime keeps long-polling there.
    pub fn active(&self) -> tokio::sync::watch::Receiver<bool> {
        self.active.subscribe()
    }
    /// The gateway sent a hint on this connection, so it pushes commands.
    pub fn received(&self) {
        self.active.send_replace(true);
        self.hint();
    }
    /// A gateway connection opened. Commands may have arrived while it was
    /// down, so lease now. Push is not trusted until the gateway sends a hint.
    pub fn connected(&self) {
        self.active.send_replace(false);
        self.hint();
    }
    /// The gateway connection ended: fall back to long-polling.
    pub fn disconnected(&self) {
        self.active.send_replace(false);
    }
}
/// The process-wide hints fed by this runtime's gateway attachment.
pub fn command_push() -> &'static Arc<CommandPush> {
    static PUSH: std::sync::OnceLock<Arc<CommandPush>> = std::sync::OnceLock::new();
    PUSH.get_or_init(Default::default)
}
/// Ends push mode however the gateway attachment returns.
struct PushConnection<'a>(&'a CommandPush);
impl Drop for PushConnection<'_> {
    fn drop(&mut self) {
        self.0.disconnected();
    }
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
    serve_guest_connected(service, grant, workspace, owner, cancel, None).await
}

/// Signals only after the authenticated WebSocket upgrade has succeeded.
pub async fn serve_guest_connected(
    service: Arc<dyn RpcService>,
    grant: Grant,
    workspace: &str,
    owner: &str,
    cancel: CancellationToken,
    connected: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<(), RpcError> {
    if owner.starts_with("organization:") {
        return Err(RpcError::Capability(
            "cloud.organization_access_unavailable".into(),
        ));
    }
    let mut socket = tokio::select! {biased; _=cancel.cancelled()=>return Ok(()),socket=dial(&grant,workspace)=>socket?};
    if let Some(connected) = connected {
        let _ = connected.send(());
    }
    let push = command_push();
    push.connected();
    let _push_connection = PushConnection(push);
    let mut peers = HashMap::<String, Peer>::new();
    let (output, mut outgoing) = mpsc::channel::<(String, String)>(16);
    let mut liveness = Liveness::new(std::time::Instant::now());
    let mut heartbeat = tokio::time::interval(HEARTBEAT_TICK);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        tokio::select! {biased;
            _=cancel.cancelled()=>break,
            _=heartbeat.tick()=>match liveness.tick(std::time::Instant::now()) {
                Heartbeat::Idle=>{},
                Heartbeat::Ping=>{tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Ping(Vec::new().into()))).await.map_err(|_|RpcError::Closed)?.map_err(|_|RpcError::Closed)?;}
                // A machine woken from a pause, or a peer that stopped answering,
                // holds a socket the gateway has already dropped. Reconnect now.
                Heartbeat::Reconnect=>return Err(RpcError::Closed),
            },
            incoming=socket.next()=>{liveness.received(std::time::Instant::now());match incoming {
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
                        // A hint grants no execution authority; the mailbox still leases.
                        Control::Command=>push.received(),
                        Control::RuntimeUpdatePollNow=>runtime_update_poll_requests().notify_one(),
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
            }},
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

#[cfg(test)]
mod control_tests {
    use super::*;
    #[test]
    fn command_hint_control_parses_with_or_without_extra_fields() {
        for text in [
            r#"{"type":"runtime.command"}"#,
            r#"{"type":"runtime.command","reason":"mailbox"}"#,
        ] {
            assert!(matches!(
                serde_json::from_str::<Control>(text).unwrap(),
                Control::Command
            ));
        }
    }
    #[tokio::test]
    async fn command_push_is_trusted_only_after_a_hint_on_the_current_connection() {
        let push = CommandPush::default();
        let active = push.active();
        push.connected();
        assert!(!*active.borrow(), "a new connection has not proven push");
        tokio::time::timeout(Duration::from_millis(100), push.hinted())
            .await
            .expect("connecting wakes the mailbox");
        push.received();
        assert!(*active.borrow());
        // Hints sent while nobody waits coalesce into one pending wake.
        push.hint();
        tokio::time::timeout(Duration::from_millis(100), push.hinted())
            .await
            .expect("a received hint stays pending");
        tokio::time::timeout(Duration::from_millis(100), push.hinted())
            .await
            .expect_err("hints coalesce");
        drop(PushConnection(&push));
        assert!(
            !*active.borrow(),
            "a closed connection falls back to polling"
        );
    }
    #[test]
    fn poll_now_control_matches_the_update_contract() {
        let text = format!(
            "{{\"type\":\"{}\"}}",
            zuse_protocol::cloud_runtime_updates::POLL_NOW_METHOD
        );
        assert!(matches!(
            serde_json::from_str::<Control>(&text).unwrap(),
            Control::RuntimeUpdatePollNow
        ));
    }
}
#[cfg(test)]
mod liveness_tests {
    use super::*;
    #[test]
    fn heartbeat_pings_quiet_sockets_and_reconnects_after_silence_or_a_pause() {
        let start = std::time::Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut l = Liveness::new(start);
        assert_eq!(l.tick(at(5)), Heartbeat::Idle);
        assert_eq!(l.tick(at(10)), Heartbeat::Idle);
        assert_eq!(l.tick(at(15)), Heartbeat::Ping);
        l.received(at(16));
        assert_eq!(l.tick(at(20)), Heartbeat::Idle);
        // Silence: nothing received for 45s while ticking normally.
        let mut quiet = Liveness::new(start);
        for s in (5..45).step_by(5) {
            assert_ne!(quiet.tick(at(s)), Heartbeat::Reconnect, "{s}");
        }
        assert_eq!(quiet.tick(at(45)), Heartbeat::Reconnect);
        // A paused machine: the next tick arrives 30 minutes late.
        let mut paused = Liveness::new(start);
        paused.received(at(4));
        assert_eq!(paused.tick(at(5)), Heartbeat::Idle);
        assert_eq!(paused.tick(at(5 + 1800)), Heartbeat::Reconnect);
    }
}
#[cfg(test)]
mod channel_liveness_tests {
    use super::*;
    use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};

    async fn pair() -> (
        WebSocketStream<tokio::io::DuplexStream>,
        WebSocketStream<tokio::io::DuplexStream>,
    ) {
        let (client, server) = tokio::io::duplex(1 << 16);
        (
            WebSocketStream::from_raw_socket(client, Role::Client, None).await,
            WebSocketStream::from_raw_socket(server, Role::Server, None).await,
        )
    }

    /// A desktop channel whose gateway went silent (sleep, network change)
    /// used to wait forever, so the chat's transcript froze mid-turn. It now
    /// closes after the gateway's heartbeat window and the controller
    /// reconnects.
    #[tokio::test(start_paused = true)]
    async fn silent_gateway_closes_the_desktop_channel() {
        let (client, _server) = pair().await;
        let (input, _incoming) = mpsc::channel(8);
        let (_outgoing, out) = mpsc::channel(8);
        let closed = CancellationToken::new();
        let task = tokio::spawn(pump(client, input, out, closed.clone()));
        tokio::time::sleep(SILENCE_LIMIT - Duration::from_secs(10)).await;
        assert!(
            !closed.is_cancelled(),
            "a quiet but young socket stays open"
        );
        tokio::time::timeout(Duration::from_secs(30), closed.cancelled())
            .await
            .expect("a gateway silent past its heartbeat window is closed");
        task.await.unwrap();
    }

    /// The gateway's 15s pings keep an idle channel open, and its frames
    /// still reach the controller.
    #[tokio::test(start_paused = true)]
    async fn gateway_pings_keep_an_idle_desktop_channel_open() {
        let (client, mut server) = pair().await;
        let (input, mut incoming) = mpsc::channel(8);
        let (_outgoing, out) = mpsc::channel(8);
        let closed = CancellationToken::new();
        let _task = tokio::spawn(pump(client, input, out, closed.clone()));
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(15)).await;
            server.send(Message::Ping(Vec::new().into())).await.unwrap();
            // Drain the channel's pongs and pings so the pipe never fills.
            while let Ok(Some(Ok(_))) =
                tokio::time::timeout(Duration::from_millis(1), server.next()).await
            {}
        }
        assert!(
            !closed.is_cancelled(),
            "three minutes of pings keep it open"
        );
        server
            .send(Message::Text("{\"id\":1}".into()))
            .await
            .unwrap();
        assert_eq!(incoming.recv().await.as_deref(), Some("{\"id\":1}"));
    }
}
#[cfg(test)]
mod handshake_tests {
    use super::*;
    #[test]
    fn temporary_gateway_statuses_do_not_claim_credentials_were_rejected() {
        for status in [409, 429, 500, 503, 504] {
            let error = tokio_tungstenite::tungstenite::Error::Http(
                tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(status)
                    .body(None)
                    .unwrap(),
            );
            assert!(
                handshake_error(&error)
                    .to_string()
                    .contains("temporarily unavailable")
            );
        }
        for status in [401, 403] {
            let error = tokio_tungstenite::tungstenite::Error::Http(
                tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(status)
                    .body(None)
                    .unwrap(),
            );
            assert!(
                handshake_error(&error)
                    .to_string()
                    .contains("authorization failed")
            );
        }
    }
}
