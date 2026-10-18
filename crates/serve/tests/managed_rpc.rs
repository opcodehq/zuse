//! Actual account-RPC sockets: authentication precedes dispatch and retires streams.
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use zuse_rpc::{RpcError, RpcReply, RpcService};
use zuse_serve::managed::{Authority, Authorization, PROTOCOL};
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Auth {
    account: CancellationToken,
    host: CancellationToken,
}
impl Authority for Auth {
    fn authorize(&self, token: &str) -> Option<Authorization> {
        (token == "fixture-grant" || token == "expiring-grant").then(|| Authorization {
            device_id: "host-device".into(),
            subject: "account-user".into(),
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                + 60,
            deadline: Instant::now()
                + if token == "expiring-grant" {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(60)
                },
            account_closed: self.account.clone(),
            host_closed: self.host.clone(),
        })
    }
}
struct Service {
    calls: Arc<AtomicUsize>,
    streams: Arc<AtomicUsize>,
}
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl RpcService for Service {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if method == "Watch" {
            self.streams.fetch_add(1, Ordering::SeqCst);
            let guard = Guard(self.streams.clone());
            return Ok(RpcReply::Stream(Box::pin(
                futures::stream::once(async { json!({"ready":true}) }).chain(
                    futures::stream::once(async move {
                        let _guard = guard;
                        futures::future::pending::<Value>().await
                    }),
                ),
            )));
        }
        RpcReply::value(&params)
    }
}
async fn receive(socket: &mut Socket) -> Option<Value> {
    match tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
    {
        Some(Ok(Message::Text(text))) => Some(serde_json::from_str(&text).unwrap()),
        _ => None,
    }
}
#[tokio::test]
async fn bounded_authentication_foreign_host_rejection_and_live_stream_retirement() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/api/account-rpc", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let streams = Arc::new(AtomicUsize::new(0));
    let account = CancellationToken::new();
    let host = CancellationToken::new();
    let shutdown = CancellationToken::new();
    let browser = zuse_serve::BrowserServer::new(
        Arc::new(Service {
            calls: calls.clone(),
            streams: streams.clone(),
        }),
        "x".repeat(64),
        None,
    )
    .unwrap()
    .with_managed_authority(Arc::new(Auth {
        account: account.clone(),
        host: host.clone(),
    }));
    let task = tokio::spawn(browser.serve_with_shutdown(
        listener,
        root.path().into(),
        None,
        shutdown.clone().cancelled_owned(),
    ));
    let mut hostile = url.clone().into_client_request().unwrap();
    hostile
        .headers_mut()
        .insert("origin", "https://unrelated.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(hostile).await.is_err());
    for auth in [
        json!({"protocol":PROTOCOL,"connectToken":"bad"}),
        json!({"protocol":"old-wire","connectToken":"fixture-grant"}),
        json!({"id":"1","method":"Write","params":{}}),
    ] {
        let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        socket
            .send(Message::Text(auth.to_string().into()))
            .await
            .unwrap();
        assert!(receive(&mut socket).await.is_none());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (mut expiring, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    expiring
        .send(Message::Text(
            json!({"protocol":PROTOCOL,"connectToken":"expiring-grant"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut expiring).await.unwrap()["authenticated"], true);
    assert!(receive(&mut expiring).await.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    socket
        .send(Message::Text(
            json!({"protocol":PROTOCOL,"connectToken":"fixture-grant"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut socket).await.unwrap()["authenticated"], true);
    socket
        .send(Message::Text(
            json!({"id":1,"method":"Write","params":{"targetDeviceId":"other"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let rejected = receive(&mut socket).await.unwrap();
    assert!(rejected.to_string().contains("Connect directly"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    socket
        .send(Message::Text(
            json!({"id":2,"method":"Watch","params":{}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(
        receive(&mut socket)
            .await
            .unwrap()
            .to_string()
            .contains("ready")
    );
    assert_eq!(streams.load(Ordering::SeqCst), 1);
    account.cancel();
    assert!(receive(&mut socket).await.is_none());
    tokio::time::timeout(Duration::from_secs(3), async {
        while streams.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    socket
        .send(Message::Text(
            json!({"protocol":PROTOCOL,"connectToken":"fixture-grant"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(receive(&mut socket).await.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
