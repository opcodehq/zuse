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

pub const PROTOCOL: &str = "zuse-rpc-json-v1";
/// Implemented by the engine's account-bound host owner; transport holds no key.
pub trait Authority: Send + Sync {
    fn authorize(&self, token: &str) -> Option<Authorization>;
    fn valid_origin(&self, _origin: &str) -> bool {
        false
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Authenticate {
    protocol: String,
    connect_token: String,
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
    grant: Arc<Authorization>,
}
#[async_trait::async_trait]
impl RpcService for Authorized {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if !self.grant.is_live() {
            return Err(RpcError::Failed(
                "Account connection authorization expired".into(),
            ));
        }
        if params
            .get("targetDeviceId")
            .filter(|v| !v.is_null())
            .is_some_and(|v| v.as_str() != Some(self.grant.device_id.as_str()))
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
    let (mut writer, mut reader) = socket.split();
    let (output, mut outgoing) = mpsc::channel::<String>(16);
    let (input, incoming) = mpsc::channel::<String>(16);
    let service = Arc::new(Authorized {
        inner: server.service,
        grant: grant.clone(),
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
                    Some(Ok(Message::Text(frame))) => input.send(frame.to_string()).await.map_err(|_| ())?,
                    Some(Ok(Message::Ping(data))) => writer.send(Message::Pong(data)).await.map_err(|_| ())?,
                    Some(Ok(Message::Pong(_))) => {},
                    _ => return Ok(()),
                },
            }
        }
    };
    tokio::select! {biased;
        _ = server.shutdown.cancelled() => {},
        _ = grant.account_closed.cancelled() => {},
        _ = grant.host_closed.cancelled() => {},
        _ = tokio::time::sleep_until(grant.deadline.into()) => {},
        _ = exchange => {},
    }
    drop(input);
    dispatch.abort();
    let _ = dispatch.await;
}
