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
        if ![
            "fixture-grant",
            "expiring-grant",
            "renewable-grant",
            "renewed-grant",
            "other-account-grant",
        ]
        .contains(&token)
        {
            return None;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let expires_at = now.as_secs() as i64 + if token == "renewable-grant" { 2 } else { 60 };
        Some(Authorization {
            device_id: "host-device".into(),
            subject: if token == "other-account-grant" {
                "other-user"
            } else {
                "account-user"
            }
            .into(),
            expires_at,
            deadline: Instant::now()
                + if token == "expiring-grant" {
                    Duration::from_millis(100)
                } else {
                    Duration::from_millis((expires_at * 1000 - now.as_millis() as i64) as u64)
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
            json!({"protocol":PROTOCOL,"ping":"proof"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(receive(&mut socket).await.unwrap()["pong"], "proof");
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

struct Source {
    origin: String,
    calls: AtomicUsize,
    cancel: CancellationToken,
    swap_subject: bool,
}
#[tokio::test]
async fn cross_origin_account_transport_requires_explicit_protocol_and_private_grant() {
    let host = TestHost::new(false).await;
    let url = format!(
        "{}/api/account-rpc",
        host.source.origin.replace("http://", "ws://")
    );
    let request = |url: &str, origin: &str| {
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        request
            .headers_mut()
            .insert("sec-websocket-protocol", PROTOCOL.parse().unwrap());
        request.headers_mut().insert(
            "cookie",
            format!("zuse_session={}", "x".repeat(64)).parse().unwrap(),
        );
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", "x".repeat(64)).parse().unwrap(),
        );
        request
    };
    for origin in [
        "null",
        "http://public.example",
        "https://user:password@public.example",
    ] {
        assert!(
            tokio_tungstenite::connect_async(request(&url, origin))
                .await
                .is_err()
        );
    }
    let plain = url.replace("/api/account-rpc", "/api/rpc");
    assert!(
        tokio_tungstenite::connect_async(request(&plain, "https://controller.example"))
            .await
            .is_err()
    );
    let (mut socket, _) =
        tokio_tungstenite::connect_async(request(&url, "https://controller.example"))
            .await
            .unwrap();
    socket
        .send(Message::Text(
            json!({"id":1,"method":"Write","params":{}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(receive(&mut socket).await.is_none());
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(request(&url, "https://controller.example"))
            .await
            .unwrap();
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
            json!({"id":1,"method":"Write","params":{"value":"authorized"}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive(&mut socket).await.unwrap()["ok"]["value"],
        "authorized"
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    drop(socket);
    host.close().await;
}
#[async_trait::async_trait]
impl zuse_rpc::managed::GrantSource for Source {
    fn environment(&self) -> &str {
        "host-device"
    }
    fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
    async fn grant(&self) -> Result<zuse_protocol::managed_connection::Bootstrap, RpcError> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        Ok(zuse_protocol::managed_connection::Bootstrap {
            environment_id: "host-device".into(),
            origin: self.origin.clone(),
            connect_token: if first {
                "renewable-grant"
            } else if self.swap_subject {
                "other-account-grant"
            } else {
                "renewed-grant"
            }
            .into(),
            expires_at: now + if first { 2000 } else { 60000 },
        })
    }
}
struct TestHost {
    _root: tempfile::TempDir,
    source: Arc<Source>,
    calls: Arc<AtomicUsize>,
    streams: Arc<AtomicUsize>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl TestHost {
    async fn new(swap_subject: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = Arc::new(Source {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            calls: AtomicUsize::new(0),
            cancel: CancellationToken::new(),
            swap_subject,
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let streams = Arc::new(AtomicUsize::new(0));
        let stop = CancellationToken::new();
        let server = zuse_serve::BrowserServer::new(
            Arc::new(Service {
                calls: calls.clone(),
                streams: streams.clone(),
            }),
            "x".repeat(64),
            None,
        )
        .unwrap()
        .with_managed_authority(Arc::new(Auth {
            account: CancellationToken::new(),
            host: CancellationToken::new(),
        }));
        let task = tokio::spawn(server.serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ));
        Self {
            _root: root,
            source,
            calls,
            streams,
            stop,
            task,
        }
    }
    async fn close(self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
#[tokio::test]
async fn native_grant_renewal_preserves_the_same_canonical_stream_and_cancellation_retires_it() {
    let host = TestHost::new(false).await;
    let connection = zuse_rpc::managed::connect(host.source.clone())
        .await
        .unwrap();
    let mut watch = connection
        .client
        .subscribe_scoped("Watch", json!({}))
        .await
        .unwrap();
    assert_eq!(watch.recv().await.unwrap()["ready"], true);
    assert_eq!(
        connection
            .client
            .call("Write", json!({"value":1}))
            .await
            .unwrap()["value"],
        1
    );
    // Remain beyond the initial grant's lifetime; no stream resubscription occurs.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(host.source.calls.load(Ordering::SeqCst), 2);
    assert_eq!(host.streams.load(Ordering::SeqCst), 1);
    assert_eq!(host.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        connection
            .client
            .call("Read", json!({"value":2}))
            .await
            .unwrap()["value"],
        2
    );
    host.source.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), connection.closed.cancelled())
        .await
        .unwrap();
    assert!(
        connection
            .client
            .call("Write", json!({"value":3}))
            .await
            .is_err()
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 3);
    drop(watch);
    host.close().await;
}
#[tokio::test]
async fn renewing_as_another_subject_closes_the_existing_stream_instead_of_switching_accounts() {
    let host = TestHost::new(true).await;
    let connection = zuse_rpc::managed::connect(host.source.clone())
        .await
        .unwrap();
    let mut watch = connection
        .client
        .subscribe_scoped("Watch", json!({}))
        .await
        .unwrap();
    watch.recv().await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), connection.closed.cancelled())
        .await
        .unwrap();
    assert_eq!(host.source.calls.load(Ordering::SeqCst), 2);
    assert!(connection.client.call("Write", json!({})).await.is_err());
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    drop(watch);
    host.close().await;
}
