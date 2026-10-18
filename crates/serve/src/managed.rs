//! Account-granted RPC. Authentication is a bounded first frame, never a URL.
use crate::BrowserServer;
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zuse_rpc::{RpcError, RpcReply, RpcService};

use zuse_protocol::managed_connection::Authenticate;
pub use zuse_protocol::managed_connection::{PROTOCOL, WIRE_VERSION};
/// Implemented by the engine's account-bound host owner; transport holds no key.
pub trait Authority: Send + Sync {
    fn authorize(&self, token: &str) -> Option<Authorization>;
    fn valid_origin(&self, _origin: &str) -> bool {
        false
    }
    fn listener_opened(
        &self,
        _address: std::net::SocketAddr,
        _local_http: bool,
        _closed: CancellationToken,
    ) {
    }
}
/// Both account and host lifetimes must remain live for the entire connection.
pub struct Authorization {
    pub device_id: String,
    pub subject: String,
    pub expires_at: i64,
    pub deadline: Instant,
    pub account_closed: CancellationToken,
    pub host_closed: CancellationToken,
}
impl Authorization {
    pub fn is_live(&self) -> bool {
        !self.account_closed.is_cancelled()
            && !self.host_closed.is_cancelled()
            && Instant::now() < self.deadline
            && self.expires_at > 0
            && std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .is_ok_and(|now| now.as_secs() < self.expires_at as u64)
    }
}
pub(super) async fn upgrade(
    State(server): State<BrowserServer>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if server.managed.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Native clients need no Origin. Browsers must use this listener's origin.
    if headers.contains_key(header::ORIGIN)
        && !server.valid_origin(&headers)
        && !headers
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|origin| {
                server
                    .managed
                    .as_ref()
                    .is_some_and(|a| a.valid_origin(origin))
            })
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = server.paired_slots.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let connections = server.connections.clone();
    ws.protocols([PROTOCOL])
        .max_message_size(2 * 1024 * 1024)
        .on_upgrade(move |socket| {
            connections.track_future(async move {
                let _permit = permit;
                session(server, socket).await;
            })
        })
}
struct Authorized {
    inner: Arc<dyn RpcService>,
    grant: tokio::sync::watch::Receiver<Arc<Authorization>>,
}
#[async_trait::async_trait]
impl RpcService for Authorized {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        let grant = self.grant.borrow().clone();
        if !grant.is_live() {
            return Err(RpcError::Failed(
                "Account connection authorization expired".into(),
            ));
        }
        if params
            .get("targetDeviceId")
            .filter(|v| !v.is_null())
            .is_some_and(|v| v.as_str() != Some(grant.device_id.as_str()))
        {
            return Err(RpcError::BadParams(
                "Connect directly to the execution host".into(),
            ));
        }
        self.inner.handle(method, params).await
    }
}
async fn session(server: BrowserServer, mut socket: WebSocket) {
    let authenticate = async {
        let Some(Ok(Message::Text(frame))) = socket.next().await else {
            return None;
        };
        if frame.len() > 20 * 1024 {
            return None;
        }
        let auth: Authenticate = serde_json::from_str(&frame).ok()?;
        if auth.protocol != PROTOCOL || auth.connect_token.len() > 16384 {
            return None;
        }
        server.managed.as_ref()?.authorize(&auth.connect_token)
    };
    let grant = tokio::select! {biased;
        _ = server.shutdown.cancelled() => return,
        result = tokio::time::timeout(Duration::from_secs(10), authenticate) => {
            let Ok(Some(grant)) = result else { let _ = socket.close().await; return; };
            Arc::new(grant)
        }
    };
    if !grant.is_live() {
        return;
    }
    let acknowledgement = serde_json::json!({"authenticated":true,"protocol":PROTOCOL,"deviceId":grant.device_id,"expiresAt":grant.expires_at*1000}).to_string();
    let acknowledge = socket.send(Message::Text(acknowledgement.into()));
    tokio::select! {biased;
        _ = server.shutdown.cancelled() => return,
        _ = grant.account_closed.cancelled() => return,
        _ = grant.host_closed.cancelled() => return,
        _ = tokio::time::sleep_until(grant.deadline.into()) => return,
        result = tokio::time::timeout(Duration::from_secs(10), acknowledge) => {
            if !matches!(result, Ok(Ok(()))) {return;}
        }
    }
    let authority = server.managed.clone().unwrap();
    let (grants, current) = tokio::sync::watch::channel(grant);
    let lifetime = current.clone();
    let (mut writer, mut reader) = socket.split();
    let (output, mut outgoing) = mpsc::channel::<String>(16);
    let (input, incoming) = mpsc::channel::<String>(16);
    let service = Arc::new(Authorized {
        inner: server.service,
        grant: current,
    });
    let dispatch = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
        zuse_rpc::serve_connection(service, output, incoming),
    ));
    let exchange = async {
        loop {
            tokio::select! {
                frame = outgoing.recv() => match frame {
                    Some(frame) => writer.send(Message::Text(frame.into())).await.map_err(|_| ())?,
                    None => return Ok::<_, ()>(()),
                },
                frame = reader.next() => match frame {
                    Some(Ok(Message::Text(frame))) => {
                        #[derive(Deserialize)]
                        struct Probe<'a> {#[serde(borrow)] protocol:Option<&'a str>,#[serde(borrow)] ping:Option<&'a str>}
                        // Borrow only control fields; large RPC params are not cloned.
                        if let Ok(probe)=serde_json::from_str::<Probe>(&frame) {
                            if let Some(protocol)=probe.protocol {
                                if protocol!=PROTOCOL {return Err(());}
                                if let Some(nonce)=probe.ping {
                                    if nonce.is_empty() || nonce.len()>128 {return Err(());}
                                    let pong=serde_json::json!({"protocol":PROTOCOL,"pong":nonce}).to_string();
                                    writer.send(Message::Text(pong.into())).await.map_err(|_|())?;
                                    continue;
                                }
                                if frame.len()>20*1024 {return Err(());}
                                let auth:Authenticate=serde_json::from_str(&frame).map_err(|_|())?;
                                let next=authority.authorize(&auth.connect_token).ok_or(())?;
                                let previous=grants.borrow().clone();
                                if !previous.is_live() || !next.is_live() || next.device_id!=previous.device_id || next.subject!=previous.subject {return Err(());}
                                let acknowledgement=serde_json::json!({"authenticated":true,"protocol":PROTOCOL,"deviceId":next.device_id,"expiresAt":next.expires_at*1000,"renewed":true}).to_string();
                                grants.send_replace(Arc::new(next));
                                writer.send(Message::Text(acknowledgement.into())).await.map_err(|_|())?;
                                continue;
                            }
                        }
                        input.send(frame.to_string()).await.map_err(|_| ())?;
                    },
                    Some(Ok(Message::Ping(data))) => writer.send(Message::Pong(data)).await.map_err(|_| ())?,
                    Some(Ok(Message::Pong(_))) => {},
                    _ => return Ok(()),
                },
            }
        }
    };
    tokio::select! {biased;
        _ = server.shutdown.cancelled() => {},
        _ = authorization_lifetime(lifetime) => {},
        _ = exchange => {},
    }
    drop(input);
    dispatch.abort();
    let _ = dispatch.await;
}
async fn authorization_lifetime(mut grants: tokio::sync::watch::Receiver<Arc<Authorization>>) {
    loop {
        let grant = grants.borrow_and_update().clone();
        if !grant.is_live() {
            return;
        }
        tokio::select! {biased;
            _=grant.account_closed.cancelled()=>return,
            _=grant.host_closed.cancelled()=>return,
            _=tokio::time::sleep_until(grant.deadline.into())=>return,
            result=grants.changed()=>if result.is_err(){return;},
            _=tokio::time::sleep(Duration::from_secs(1))=>{},
        }
    }
}
