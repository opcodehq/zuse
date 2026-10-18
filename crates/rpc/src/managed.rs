//! Explicit account sockets with in-place grant renewal and no action replay.
use crate::{RpcClient, RpcError};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::Instant};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use zuse_protocol::managed_connection::{Bootstrap, Control, PROTOCOL, renew_in_ms};
#[async_trait::async_trait]
pub trait GrantSource: Send + Sync {
    fn environment(&self) -> &str;
    fn cancellation(&self) -> CancellationToken;
    async fn grant(&self) -> Result<Bootstrap, RpcError>;
}
fn invalid() -> RpcError {
    RpcError::Transport("Account connection authorization failed".into())
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
fn expiry(at: i64) -> Result<Instant, RpcError> {
    let left = at
        .checked_sub(now())
        .filter(|left| *left > 0 && *left <= 86400000)
        .ok_or_else(invalid)?;
    Ok(Instant::now() + Duration::from_millis(left as u64))
}
fn socket_url(grant: &Bootstrap, expected: &str) -> Result<String, RpcError> {
    if grant.environment_id != expected
        || grant.connect_token.is_empty()
        || grant.connect_token.len() > 16384
    {
        return Err(invalid());
    }
    expiry(grant.expires_at)?;
    let origin = zuse_remote::normalized_origin(&grant.origin).map_err(|_| invalid())?;
    if origin != grant.origin.trim_end_matches('/') {
        return Err(invalid());
    }
    let socket = if let Some(host) = origin.strip_prefix("https://") {
        format!("wss://{host}")
    } else {
        format!(
            "ws://{}",
            origin.strip_prefix("http://").ok_or_else(invalid)?
        )
    };
    Ok(format!("{socket}/api/account-rpc"))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ack {
    authenticated: bool,
    protocol: String,
    device_id: String,
    expires_at: i64,
    #[serde(default)]
    renewed: bool,
}
fn acknowledge(text: &str, grant: &Bootstrap, renewed: bool) -> Result<i64, RpcError> {
    let ack: Ack = serde_json::from_str(text).map_err(|_| invalid())?;
    if !ack.authenticated
        || ack.protocol != PROTOCOL
        || ack.device_id != grant.environment_id
        || ack.renewed != renewed
        || ack.expires_at > grant.expires_at.saturating_add(1000)
    {
        return Err(invalid());
    }
    expiry(ack.expires_at)?;
    Ok(ack.expires_at)
}
fn auth(grant: &Bootstrap) -> Message {
    Message::Text(
        json!({"protocol":PROTOCOL,"connectToken":grant.connect_token})
            .to_string()
            .into(),
    )
}
async fn get_grant(source: Arc<dyn GrantSource>) -> Result<Bootstrap, RpcError> {
    let cancel = source.cancellation();
    tokio::select! {biased;
        _=cancel.cancelled()=>Err(RpcError::Closed),
        value=tokio::time::timeout(Duration::from_secs(60),source.grant())=>value.map_err(|_|RpcError::Transport("Account authorization timed out".into()))?,
    }
}
pub struct Connection {
    pub client: RpcClient,
    pub closed: CancellationToken,
}

/// A socket remains bound to the selected environment. Reconnect is explicit;
/// this transport never resends a canonical RPC frame after a connection loss.
pub async fn connect(source: Arc<dyn GrantSource>) -> Result<Connection, RpcError> {
    let channel = connect_channel(source).await?;
    Ok(Connection {
        client: RpcClient::new(channel.outgoing, channel.incoming),
        closed: channel.closed,
    })
}
pub async fn connect_channel(
    source: Arc<dyn GrantSource>,
) -> Result<crate::reconnect::Channel, RpcError> {
    let cancel = source.cancellation();
    let grant = get_grant(source.clone()).await?;
    let url = socket_url(&grant, source.environment())?;
    let first = async {
        let config = WebSocketConfig {
            max_message_size: Some(2 * 1024 * 1024),
            max_frame_size: Some(2 * 1024 * 1024),
            ..Default::default()
        };
        let (mut socket, _) = connect_async_with_config(&url, Some(config), false)
            .await
            .map_err(|_| RpcError::Transport("Could not reach the selected computer".into()))?;
        socket
            .send(auth(&grant))
            .await
            .map_err(|_| RpcError::Closed)?;
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let at = acknowledge(&text, &grant, false)?;
                    return Ok::<_, RpcError>((socket, at));
                }
                Some(Ok(Message::Ping(bytes))) => socket
                    .send(Message::Pong(bytes))
                    .await
                    .map_err(|_| RpcError::Closed)?,
                Some(Ok(Message::Pong(_))) => {}
                _ => return Err(invalid()),
            }
        }
    };
    let (socket, at) = tokio::select! {biased;_=cancel.cancelled()=>return Err(RpcError::Closed),value=tokio::time::timeout(Duration::from_secs(10),first)=>value.map_err(|_|RpcError::Transport("Account connection timed out".into()))??};
    let (output, mut outgoing) = mpsc::channel::<String>(16);
    let (input, incoming) = mpsc::channel::<String>(16);
    let initial_deadline = expiry(at)?;
    let closed = CancellationToken::new();
    let completion = closed.clone();
    tokio::spawn(async move {
        let _completed = completion.drop_guard();
        let (mut writer, mut reader) = socket.split();
        let (deadlines, mut deadline) = tokio::sync::watch::channel(initial_deadline);
        let lifetime = async {
            loop {
                let at = *deadline.borrow_and_update();
                tokio::select! {biased;_=tokio::time::sleep_until(at)=>return,value=deadline.changed()=>if value.is_err(){return;}}
            }
        };
        let exchange = async {
            let mut renew_at =
                Instant::now() + Duration::from_millis(renew_in_ms(at, now()).unwrap_or(1));
            let mut renewal: Option<AbortOnDropHandle<Result<Bootstrap, RpcError>>> = None;
            let mut awaiting: Option<(Bootstrap, Instant)> = None;
            let mut failures = 0u32;
            let mut valid_until = at;
            let mut ping_at = Instant::now() + Duration::from_secs(20);
            let mut pong: Option<(String, Instant)> = None;
            loop {
                tokio::select! {
                    _=tokio::time::sleep_until(awaiting.as_ref().map(|(_,at)|*at).unwrap_or_else(||Instant::now()+Duration::from_secs(3600))), if awaiting.is_some()=>return Err(()),
                    _=tokio::time::sleep_until(pong.as_ref().map(|(_,at)|*at).unwrap_or_else(||Instant::now()+Duration::from_secs(3600))), if pong.is_some()=>return Err(()),
                    result=async {renewal.as_mut().unwrap().await}, if renewal.is_some()=>{
                        renewal=None;
                        match result {
                            Ok(Ok(grant))=>{
                                let next=socket_url(&grant,source.environment()).map_err(|_|())?;
                                // A hostname change needs a new explicit socket, never an implicit route switch.
                                if next!=url {return Err(());}
                                if grant.expires_at<=valid_until.saturating_add(1000) {
                                    failures=failures.saturating_add(1);renew_at=Instant::now()+Duration::from_secs((1u64<<failures.min(5)).min(30));continue;
                                }
                                writer.send(auth(&grant)).await.map_err(|_|())?;
                                awaiting=Some((grant,Instant::now()+Duration::from_secs(10)));
                            },
                            _=>{failures=failures.saturating_add(1);renew_at=Instant::now()+Duration::from_secs((1u64<<failures.min(5)).min(30));},
                        }
                    },
                    _=tokio::time::sleep_until(renew_at), if renewal.is_none() && awaiting.is_none()=>{
                        let source=source.clone();renewal=Some(AbortOnDropHandle::new(tokio::spawn(async move {get_grant(source).await})));
                    },
                    _=tokio::time::sleep_until(ping_at), if pong.is_none()=>{
                        let nonce=uuid::Uuid::new_v4().to_string();
                        writer.send(Message::Text(json!({"protocol":PROTOCOL,"ping":nonce}).to_string().into())).await.map_err(|_|())?;
                        pong=Some((nonce,Instant::now()+Duration::from_secs(10)));
                    },
                    frame=outgoing.recv()=>match frame {
                        Some(frame)=>writer.send(Message::Text(frame.into())).await.map_err(|_|())?,
                        None=>return Ok::<_,()>(()),
                    },
                    frame=reader.next()=>match frame {
                        Some(Ok(Message::Text(text)))=>{
                            if let Ok(control)=serde_json::from_str::<Control>(&text) {
                                if let Some(protocol)=control.protocol {
                                    if protocol!=PROTOCOL {return Err(());}
                                    if let Some(nonce)=control.pong {
                                        if pong.as_ref().is_none_or(|(expected,_)|expected!=nonce){return Err(());}
                                        pong=None;ping_at=Instant::now()+Duration::from_secs(20);continue;
                                    }
                                    let (grant,_)=awaiting.take().ok_or(())?;
                                    let at=acknowledge(&text,&grant,true).map_err(|_|())?;
                                    if at<=valid_until{return Err(());}valid_until=at;
                                    deadlines.send_replace(expiry(at).map_err(|_|())?);
                                    renew_at=Instant::now()+Duration::from_millis(renew_in_ms(at,now()).ok_or(())?);failures=0;
                                    continue;
                                }
                            }
                            input.send(text.to_string()).await.map_err(|_|())?;
                        },
                        Some(Ok(Message::Ping(bytes)))=>writer.send(Message::Pong(bytes)).await.map_err(|_|())?,
                        Some(Ok(Message::Pong(_)))=>{},
                        _=>return Err(()),
                    }
                }
            }
        };
        tokio::select! {biased;_=cancel.cancelled()=>{},_=lifetime=>{},_=exchange=>{}}
    });
    Ok(crate::reconnect::Channel {
        outgoing: output,
        incoming,
        closed,
    })
}
