//! The real broker routes opaque records between outbound hosts and native RPC.
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use zuse_remote::{Invitation, PairedTarget, authority::Authority};
use zuse_rpc::{RpcError, RpcReply, RpcService};
use zuse_serve::{
    BrowserServer,
    relay::{HostToken, RelayBroker, serve_host},
};
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
struct Fixture {
    root: tempfile::TempDir,
    authority: Authority,
    invitation: Invitation,
    origin: String,
    token: HostToken,
    calls: Arc<AtomicUsize>,
    subscriptions: Arc<AtomicUsize>,
    stop: CancellationToken,
    broker: tokio::task::JoinHandle<anyhow::Result<()>>,
    host: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let authority = Authority::open(root.path()).unwrap();
        let (_, invitation) = authority.create("Relay client").unwrap();
        let token = HostToken::parse(BrowserServer::random_token()).unwrap();
        let stop = CancellationToken::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let broker = tokio::spawn(RelayBroker::new(token.clone()).serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            stop.clone().cancelled_owned(),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let subscriptions = Arc::new(AtomicUsize::new(0));
        let server = BrowserServer::new(
            Arc::new(Echo(calls.clone(), subscriptions.clone())),
            BrowserServer::random_token(),
            None,
        )
        .unwrap()
        .with_pairings(authority.clone(), "host-device")
        .with_shutdown_token(stop.clone());
        let relay_origin = origin.clone();
        let host_token = token.clone();
        let host = tokio::spawn(async move { serve_host(server, &relay_origin, host_token).await });
        Self {
            root,
            authority,
            invitation,
            origin,
            token,
            calls,
            subscriptions,
            stop,
            broker,
            host,
        }
    }
    fn target(&self) -> PairedTarget {
        PairedTarget::new(&self.origin, self.invitation.clone()).unwrap()
    }
    async fn client(&self) -> zuse_rpc::RpcClient {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match zuse_rpc::connect_paired(&self.target()).await {
                    Ok(client) => return client,
                    Err(_) => tokio::time::sleep(Duration::from_millis(30)).await,
                }
            }
        })
        .await
        .unwrap()
    }
    async fn close(self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.host)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), self.broker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
#[tokio::test]
async fn outbound_hosts_support_concurrent_rpc_large_records_subscriptions_and_revocation() {
    let fixture = Fixture::new().await;
    let first = fixture.client().await;
    let second = fixture.client().await;
    let payload = json!({"text":"x".repeat(180_000)});
    let (a, b) = tokio::join!(
        first.call("Echo", payload.clone()),
        second.call("Echo", json!({"agent":2}))
    );
    assert_eq!(a.unwrap(), payload);
    assert_eq!(b.unwrap()["agent"], 2);
    assert!(fixture.authority.list().unwrap()[0].paired_at.is_some());
    let mut watch = first.subscribe_scoped("Watch", json!({})).await.unwrap();
    assert_eq!(watch.recv().await.unwrap()["ready"], true);
    assert!(
        first
            .call("Echo", json!({"targetDeviceId":"foreign"}))
            .await
            .is_err()
    );
    let count = fixture.calls.load(Ordering::SeqCst);
    fixture.authority.revoke(&fixture.invitation.id).unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            second.call("Echo", json!({"denied":true}))
        )
        .await
        .unwrap()
        .is_err()
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), count);
    drop(watch);
    drop(first);
    drop(second);
    fixture.close().await;
}
#[tokio::test]
async fn broker_rejects_unregistered_hosts_plaintext_and_wrong_pairing_keys() {
    let fixture = Fixture::new().await;
    let mut request = format!(
        "{}/api/paired-host/{}",
        fixture.origin.replace("http:", "ws:"),
        fixture.invitation.id.as_str()
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer invalid".parse().unwrap());
    assert!(
        matches!(connect_async(request).await,Err(tokio_tungstenite::tungstenite::Error::Http(response))if response.status()==401)
    );
    let client = fixture.client().await;
    assert_eq!(client.call("Echo", json!({})).await.unwrap(), json!({}));
    drop(client);
    let mut wrong = Invitation::generate().unwrap();
    wrong.id = fixture.invitation.id.clone();
    let target = PairedTarget::new(&fixture.origin, wrong).unwrap();
    assert!(zuse_rpc::connect_paired(&target).await.is_err());
    let (mut socket, _) = connect_async(fixture.target().socket_url()).await.unwrap();
    socket
        .send(Message::Text("{\"id\":1,\"method\":\"Echo\"}".into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(frame) = socket.next().await {
            if frame.is_err() || matches!(frame, Ok(Message::Close(_))) {
                return;
            }
            assert!(!matches!(
                frame,
                Ok(Message::Text(_)) | Ok(Message::Binary(_))
            ));
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    fixture.close().await;
}
#[tokio::test]
async fn host_connector_restart_uses_fresh_noise_and_explicit_shutdown_closes_clients() {
    let mut fixture = Fixture::new().await;
    let client = fixture.client().await;
    assert!(client.call("Echo", json!({"first":true})).await.is_ok());
    let mut subscription = client.subscribe_scoped("Watch", json!({})).await.unwrap();
    assert_eq!(subscription.recv().await.unwrap()["ready"], true);
    assert_eq!(fixture.subscriptions.load(Ordering::SeqCst), 1);
    fixture.host.abort();
    let _ = (&mut fixture.host).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), client.call("Echo", json!({})))
            .await
            .unwrap()
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.subscriptions.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("connector cancellation must release canonical subscriptions");
    drop(subscription);
    let server = BrowserServer::new(
        Arc::new(Echo(fixture.calls.clone(), fixture.subscriptions.clone())),
        BrowserServer::random_token(),
        None,
    )
    .unwrap()
    .with_pairings(fixture.authority.clone(), "host-device")
    .with_shutdown_token(fixture.stop.clone());
    let origin = fixture.origin.clone();
    let token = fixture.token.clone();
    let host = tokio::spawn(async move { serve_host(server, &origin, token).await });
    let replacement = fixture.client().await;
    assert_eq!(
        replacement
            .call("Echo", json!({"second":true}))
            .await
            .unwrap()["second"],
        true
    );
    fixture.stop.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), replacement.call("Echo", json!({})))
            .await
            .unwrap()
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(3), host)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), fixture.broker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(fixture.root);
}
