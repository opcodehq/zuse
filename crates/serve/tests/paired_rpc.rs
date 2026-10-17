//! Real binary WebSocket sessions use the production authority and RPC dispatcher.
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use zuse_remote::{Invitation, SecureChannel, authority::Authority};
use zuse_rpc::{RpcError, RpcReply, RpcService};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
struct Echo(Arc<AtomicUsize>, Arc<AtomicUsize>);
struct Subscription(Arc<AtomicUsize>);
impl Drop for Subscription {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl RpcService for Echo {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if method == "Watch" {
            self.1.fetch_add(1, Ordering::SeqCst);
            let guard = Subscription(self.1.clone());
            let tail = futures::stream::once(async move {
                let _guard = guard;
                futures::future::pending::<Value>().await
            });
            return Ok(RpcReply::Stream(Box::pin(
                futures::stream::once(async { json!({"ready":true}) }).chain(tail),
            )));
        }
        RpcReply::value(&params)
    }
}
struct Host {
    _root: tempfile::TempDir,
    authority: Authority,
    invitation: Invitation,
    url: String,
    calls: Arc<AtomicUsize>,
    subscriptions: Arc<AtomicUsize>,
    shutdown: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl Host {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let authority = Authority::open(root.path()).unwrap();
        let (_, invitation) = authority.create("Test device").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let subscriptions = Arc::new(AtomicUsize::new(0));
        let server = zuse_serve::BrowserServer::new(
            Arc::new(Echo(calls.clone(), subscriptions.clone())),
            zuse_serve::BrowserServer::random_token(),
            None,
        )
        .unwrap()
        .with_pairings(authority.clone(), "host-device");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "ws://{}/api/paired-rpc/{}",
            listener.local_addr().unwrap(),
            invitation.id.as_str()
        );
        let shutdown = tokio_util::sync::CancellationToken::new();
        let signal = shutdown.clone();
        let assets = root.path().to_path_buf();
        let task = tokio::spawn(server.serve_with_shutdown(
            listener,
            assets,
            None,
            signal.cancelled_owned(),
        ));
        Self {
            _root: root,
            authority,
            invitation,
            url,
            calls,
            subscriptions,
            shutdown,
            task,
        }
    }
    async fn connect(&self) -> (Socket, SecureChannel) {
        let (mut socket, _) = tokio_tungstenite::connect_async(&self.url).await.unwrap();
        let (handshake, first) =
            zuse_remote::initiate(&self.invitation.id, &self.invitation.secret).unwrap();
        socket.send(Message::Binary(first)).await.unwrap();
        let reply = receive(&mut socket).await;
        (socket, handshake.finish(&reply).unwrap())
    }
    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
async fn receive(socket: &mut Socket) -> Vec<u8> {
    let result = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    match result {
        Some(Ok(Message::Binary(bytes))) => bytes,
        other => panic!("expected encrypted binary record: {other:?}"),
    }
}
async fn send(socket: &mut Socket, channel: &mut SecureChannel, value: Value) {
    for record in channel.seal(&serde_json::to_vec(&value).unwrap()).unwrap() {
        socket.send(Message::Binary(record)).await.unwrap();
    }
}
async fn reply(socket: &mut Socket, channel: &mut SecureChannel) -> Value {
    loop {
        if let Some(bytes) = channel.open(&receive(socket).await).unwrap() {
            return serde_json::from_slice(&bytes).unwrap();
        }
    }
}
async fn closed(socket: &mut Socket) {
    let result = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    assert!(matches!(
        result,
        None | Some(Err(_)) | Some(Ok(Message::Close(_)))
    ));
}

#[tokio::test]
async fn encrypted_rpc_confirms_after_application_proof_and_handles_fragmented_messages() {
    let host = Host::new().await;
    let (mut socket, mut channel) = host.connect().await;
    assert!(host.authority.list().unwrap()[0].paired_at.is_none());
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    let params = json!({"text": "private content ".repeat(10_000)});
    send(
        &mut socket,
        &mut channel,
        json!({"id":1,"method":"Echo","params":params}),
    )
    .await;
    let result = reply(&mut socket, &mut channel).await;
    assert_eq!(result["id"], 1);
    assert_eq!(result["ok"], params);
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    let paired = &host.authority.list().unwrap()[0];
    assert!(paired.paired_at.is_some() && paired.expires_at.is_none());
    // Reconnect is a fresh Noise session with the persisted capability.
    socket.close(None).await.unwrap();
    let (mut socket, mut channel) = host.connect().await;
    send(
        &mut socket,
        &mut channel,
        json!({"id":2,"method":"Echo","params":{}}),
    )
    .await;
    assert_eq!(reply(&mut socket, &mut channel).await["id"], 2);
    // Shutdown waits for upgraded paired sockets and canonical dispatch tasks.
    host.shutdown.cancel();
    closed(&mut socket).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while host.subscriptions.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    host.stop().await;
}

#[tokio::test]
async fn revocation_closes_existing_connections_and_rejects_new_connections() {
    let host = Host::new().await;
    let (mut socket, mut channel) = host.connect().await;
    send(
        &mut socket,
        &mut channel,
        json!({"id":1,"method":"Echo","params":{}}),
    )
    .await;
    reply(&mut socket, &mut channel).await;
    host.authority.revoke(&host.invitation.id).unwrap();
    closed(&mut socket).await;
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    let Err(tokio_tungstenite::tungstenite::Error::Http(response)) =
        tokio_tungstenite::connect_async(&host.url).await
    else {
        panic!("revoked peer was admitted")
    };
    assert_eq!(response.status().as_u16(), 404);
    host.stop().await;
}

#[tokio::test]
async fn wrong_keys_plaintext_and_invalid_initial_frames_never_reach_the_engine() {
    let host = Host::new().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&host.url).await.unwrap();
    let wrong = zuse_remote::PairSecret::generate().unwrap();
    let (_, first) = zuse_remote::initiate(&host.invitation.id, &wrong).unwrap();
    socket.send(Message::Binary(first)).await.unwrap();
    closed(&mut socket).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&host.url).await.unwrap();
    socket
        .send(Message::Text(r#"{"id":1,"method":"Echo"}"#.into()))
        .await
        .unwrap();
    closed(&mut socket).await;
    for frame in [json!({"id":1,"cancel":true}), json!({"not":"rpc"})] {
        let (mut socket, mut channel) = host.connect().await;
        send(&mut socket, &mut channel, frame).await;
        closed(&mut socket).await;
    }
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    assert!(host.authority.list().unwrap()[0].paired_at.is_none());
    host.stop().await;
}

#[tokio::test]
async fn paired_requests_cannot_fall_back_to_another_hosts_legacy_route() {
    let host = Host::new().await;
    let (mut socket, mut channel) = host.connect().await;
    send(
        &mut socket,
        &mut channel,
        json!({"id":1,"method":"Echo","params":{"targetDeviceId":"foreign-host"}}),
    )
    .await;
    assert!(
        reply(&mut socket, &mut channel).await["err"]
            .as_str()
            .unwrap()
            .contains("paired execution host")
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
    send(
        &mut socket,
        &mut channel,
        json!({"id":2,"method":"Echo","params":{"targetDeviceId":"host-device"}}),
    )
    .await;
    assert_eq!(
        reply(&mut socket, &mut channel).await["ok"]["targetDeviceId"],
        "host-device"
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    host.stop().await;
}

#[tokio::test]
async fn encrypted_cancel_and_revocation_release_canonical_streams() {
    let host = Host::new().await;
    let (mut socket, mut channel) = host.connect().await;
    send(
        &mut socket,
        &mut channel,
        json!({"id":1,"method":"Watch","params":{}}),
    )
    .await;
    assert_eq!(
        reply(&mut socket, &mut channel).await["item"]["ready"],
        true
    );
    assert_eq!(host.subscriptions.load(Ordering::SeqCst), 1);
    send(&mut socket, &mut channel, json!({"id":1,"cancel":true})).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while host.subscriptions.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    send(
        &mut socket,
        &mut channel,
        json!({"id":2,"method":"Watch","params":{}}),
    )
    .await;
    assert_eq!(
        reply(&mut socket, &mut channel).await["item"]["ready"],
        true
    );
    host.authority.revoke(&host.invitation.id).unwrap();
    closed(&mut socket).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while host.subscriptions.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    host.stop().await;
}
