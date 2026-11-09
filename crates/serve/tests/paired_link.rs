//! One link across real sockets: racing, moving between routes and resuming.
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use zuse_remote::{Invitation, PairedTarget, Route, RouteKind, authority::Authority};
use zuse_rpc::{RpcClient, RpcError, RpcReply, RpcService, paired_link::PairedHost};
use zuse_serve::{BrowserServer, PairedSessions};

/// `Ticks` streams whatever the test feeds; `Count` records each execution.
struct Service {
    ticks: tokio::sync::Mutex<Option<mpsc::Receiver<Value>>>,
    executions: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl RpcService for Service {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            "Ticks" => {
                let mut ticks = self.ticks.lock().await.take().unwrap();
                Ok(RpcReply::Stream(Box::pin(async_stream(move || {
                    let next = ticks.try_recv().ok();
                    next
                }))))
            }
            "Count" => {
                self.executions.fetch_add(1, Ordering::SeqCst);
                RpcReply::value(&params)
            }
            zuse_rpc::methods::ENGINE_INFO => {
                RpcReply::value(&json!({"deviceId":"host","workspaceScope":"synced"}))
            }
            _ => RpcReply::value(&params),
        }
    }
}
fn async_stream(
    mut next: impl FnMut() -> Option<Value> + Send + 'static,
) -> impl futures::Stream<Item = Value> + Send {
    futures::stream::unfold((), move |()| {
        let value = next();
        async move {
            match value {
                Some(value) => Some((value, ())),
                None => {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    Some((json!({"idle":true}), ()))
                }
            }
        }
    })
}

struct Listener {
    origin: String,
    shutdown: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}
impl Listener {
    async fn stop(self) {
        self.shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.task).await;
    }
}
struct Host {
    root: tempfile::TempDir,
    authority: Authority,
    invitation: Invitation,
    service: Arc<Service>,
    sessions: Arc<PairedSessions>,
    routes: tokio::sync::watch::Sender<Vec<Route>>,
    executions: Arc<AtomicUsize>,
    ticks: mpsc::Sender<Value>,
}
impl Host {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let authority = Authority::open(root.path()).unwrap();
        let (_, invitation) = authority.create("Phone").unwrap();
        let executions = Arc::new(AtomicUsize::new(0));
        let (ticks, receiver) = mpsc::channel(1024);
        Self {
            root,
            authority,
            invitation,
            service: Arc::new(Service {
                ticks: tokio::sync::Mutex::new(Some(receiver)),
                executions: executions.clone(),
            }),
            sessions: Arc::default(),
            routes: tokio::sync::watch::Sender::new(vec![]),
            executions,
            ticks,
        }
    }
    async fn listen(&self) -> Listener {
        let server = BrowserServer::new(self.service.clone(), BrowserServer::random_token(), None)
            .unwrap()
            .with_pairings(self.authority.clone(), "host")
            .with_paired_sessions(self.sessions.clone())
            .with_advertised_routes(self.routes.subscribe());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(server.serve_with_shutdown(
            listener,
            self.root.path().to_path_buf(),
            None,
            shutdown.clone().cancelled_owned(),
        ));
        Listener {
            origin,
            shutdown,
            task,
        }
    }
    fn paired(&self, origin: &str) -> PairedHost {
        PairedHost::new(
            &PairedTarget::for_route(origin, self.invitation.clone()).unwrap(),
            None,
        )
    }
}
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), future)
        .await
        .expect("timed out")
}
async fn wait_for(mut condition: impl FnMut() -> bool) {
    within(async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}
fn client(channel: zuse_rpc::reconnect::Channel) -> RpcClient {
    RpcClient::new(channel.outgoing, channel.incoming)
}

#[tokio::test]
async fn connects_with_identity_compression_and_learned_routes() {
    let host = Host::new().await;
    let listener = host.listen().await;
    let learned = Route {
        kind: RouteKind::Lan,
        origin: "http://192.0.2.9:27657".replace("192.0.2.9", "10.77.0.9"),
    };
    host.routes.send_replace(vec![learned.clone()]);
    let paired = host.paired(&listener.origin);
    let status = paired.status();
    let channel = within(paired.connect(CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(channel.engine_info.as_ref().unwrap()["deviceId"], "host");
    assert!(paired.routes().contains(&learned));
    let live = status.borrow().clone().unwrap();
    assert!(live.resumable && live.compressed);
    assert_eq!(live.route.kind, RouteKind::Loopback);
    let rpc = client(channel);
    let large = json!({"text":"streamed transcript text ".repeat(4_000)});
    assert_eq!(
        within(rpc.call("Echo", large.clone())).await.unwrap(),
        large
    );
    listener.stop().await;
}

#[tokio::test]
async fn reported_routes_reach_a_connected_client() {
    let host = Host::new().await;
    let listener = host.listen().await;
    let paired = host.paired(&listener.origin);
    let _channel = within(paired.connect(CancellationToken::new()))
        .await
        .unwrap();
    let learned = Route {
        kind: RouteKind::Tailnet,
        origin: "http://100.100.3.3:27657".into(),
    };
    host.routes.send_replace(vec![learned.clone()]);
    wait_for(|| paired.routes().contains(&learned)).await;
    listener.stop().await;
}

#[tokio::test]
async fn a_lost_socket_resumes_on_another_route_without_gaps_or_repeats() {
    let host = Host::new().await;
    let first = host.listen().await;
    let second = host.listen().await;
    host.routes.send_replace(vec![Route {
        kind: RouteKind::Lan,
        origin: second.origin.clone(),
    }]);
    let paired = host.paired(&first.origin);
    let status = paired.status();
    let channel = within(paired.connect(CancellationToken::new()))
        .await
        .unwrap();
    let rpc = client(channel);
    let mut ticks = within(rpc.subscribe_scoped("Ticks", json!({})))
        .await
        .unwrap();
    for index in 0..50 {
        host.ticks.send(json!({"n":index})).await.unwrap();
    }
    let mut next = 0;
    let mut read_until = async |limit: i64| {
        while next < limit {
            let item = within(ticks.recv()).await.unwrap();
            if let Some(n) = item["n"].as_i64() {
                assert_eq!(n, next, "stream skipped or repeated an item");
                next += 1;
            }
        }
    };
    read_until(50).await;

    first.stop().await;
    for index in 50..100 {
        host.ticks.send(json!({"n":index})).await.unwrap();
    }
    assert_eq!(
        within(rpc.call("Count", json!({"after":"move"})))
            .await
            .unwrap(),
        json!({"after":"move"})
    );
    read_until(100).await;
    assert_eq!(host.executions.load(Ordering::SeqCst), 1);
    wait_for(|| {
        status
            .borrow()
            .as_ref()
            .is_some_and(|s| s.route.origin == second.origin)
    })
    .await;
    assert_eq!(host.sessions.len(), 1);
    second.stop().await;
}

#[tokio::test]
async fn revocation_ends_a_session_and_refuses_resume() {
    let host = Host::new().await;
    let listener = host.listen().await;
    let paired = host.paired(&listener.origin);
    let channel = within(paired.connect(CancellationToken::new()))
        .await
        .unwrap();
    let closed = channel.closed.clone();
    let rpc = client(channel);
    within(rpc.call("Count", json!({}))).await.unwrap();
    host.authority.revoke(&host.invitation.id).unwrap();
    wait_for(|| host.sessions.is_empty()).await;
    // Resume cannot succeed: the listener refuses the revoked pairing.
    tokio::time::timeout(Duration::from_secs(70), closed.cancelled())
        .await
        .expect("channel must close once resume is impossible");
    listener.stop().await;
}

#[tokio::test]
async fn a_wrong_route_is_skipped_for_one_that_authenticates() {
    let host = Host::new().await;
    let good = host.listen().await;
    let other = Host::new().await;
    let foreign = other.listen().await;
    // The saved first address now belongs to another machine.
    let paired = host.paired(&foreign.origin);
    host.routes.send_replace(vec![]);
    let table_route = Route {
        kind: RouteKind::Lan,
        origin: good.origin.clone(),
    };
    let saved = root_with_learned(&host, &table_route);
    let paired_with_memory = PairedHost::new(
        &PairedTarget::for_route(&foreign.origin, host.invitation.clone()).unwrap(),
        Some(saved.path()),
    );
    drop(paired);
    let channel = within(paired_with_memory.connect(CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        within(client(channel).call("Echo", json!(1)))
            .await
            .unwrap(),
        json!(1)
    );
    good.stop().await;
    foreign.stop().await;
}
fn root_with_learned(host: &Host, route: &Route) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let saved = json!({"learned":[route],"bestByNetwork":{}});
    std::fs::write(
        directory
            .path()
            .join(format!("{}.json", host.invitation.id.as_str())),
        saved.to_string(),
    )
    .unwrap();
    directory
}

#[tokio::test]
async fn a_new_client_still_works_with_a_host_that_predates_sessions() {
    // An older host answers the attach request as an ordinary unknown method.
    let root = tempfile::tempdir().unwrap();
    let invitation = Invitation::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let secret = invitation.clone();
    let peer = tokio::spawn(async move {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let Message::Binary(first) = socket.next().await.unwrap().unwrap() else {
            panic!("handshake")
        };
        let (mut channel, reply) = zuse_remote::accept(&secret.id, &secret.secret, &first).unwrap();
        socket.send(Message::Binary(reply)).await.unwrap();
        while let Some(Ok(Message::Binary(record))) = socket.next().await {
            let Some(bytes) = channel.open(&record).unwrap() else {
                continue;
            };
            let reply = if let Some(reply) = zuse_remote::heartbeat::response(&bytes).unwrap() {
                reply
            } else {
                let frame: zuse_rpc::ClientFrame = serde_json::from_slice(&bytes).unwrap();
                match frame.method.as_deref() {
                    Some("Echo") => serde_json::to_vec(&json!({"id":frame.id,"ok":frame.params})).unwrap(),
                    method => serde_json::to_vec(&json!({"id":frame.id,"err":format!("unknown method: {}", method.unwrap_or(""))})).unwrap(),
                }
            };
            for record in channel.seal(&reply).unwrap() {
                socket.send(Message::Binary(record)).await.unwrap();
            }
        }
    });
    let paired = PairedHost::new(
        &PairedTarget::for_route(&origin, invitation).unwrap(),
        Some(root.path()),
    );
    let status = paired.status();
    let channel = within(paired.connect(CancellationToken::new()))
        .await
        .unwrap();
    assert!(channel.engine_info.is_none());
    assert!(!status.borrow().as_ref().unwrap().resumable);
    assert_eq!(
        within(client(channel).call("Echo", json!(7)))
            .await
            .unwrap(),
        json!(7)
    );
    peer.abort();
}

#[tokio::test]
async fn a_connected_client_moves_itself_to_a_better_route() {
    let host = Host::new().await;
    let first = host.listen().await;
    let better = host.listen().await;
    let lan = Route {
        kind: RouteKind::Lan,
        origin: better.origin.clone(),
    };
    host.routes.send_replace(vec![lan.clone()]);
    let paired = PairedHost::with_route(
        Route {
            kind: RouteKind::Relay,
            origin: first.origin.clone(),
        },
        host.invitation.clone(),
        None,
    );
    let status = paired.status();
    let channel = within(paired.connect(CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        status.borrow().as_ref().unwrap().route.kind,
        RouteKind::Relay
    );
    let rpc = client(channel);
    let mut ticks = within(rpc.subscribe_scoped("Ticks", json!({})))
        .await
        .unwrap();
    let (seen_tx, mut seen) = mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        while let Some(item) = ticks.recv().await {
            if let Some(n) = item["n"].as_i64() {
                let _ = seen_tx.send(n);
            }
        }
    });
    host.ticks.send(json!({"n":0})).await.unwrap();
    assert_eq!(within(seen.recv()).await, Some(0));
    wait_for(|| status.borrow().as_ref().is_some_and(|s| s.route == lan)).await;
    // The subscription carried on across the move.
    host.ticks.send(json!({"n":1})).await.unwrap();
    assert_eq!(within(seen.recv()).await, Some(1));
    reader.abort();
    assert_eq!(host.sessions.len(), 1);
    first.stop().await;
    better.stop().await;
}

/// Runs the apps' state machine over real sockets, as Swift and Kotlin do.
mod sans_io {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use std::collections::HashMap;
    use tokio_tungstenite::tungstenite::Message;
    use zuse_remote::link::{Effects, Link, LinkEnd, LinkState, LinkStatus};

    enum Event {
        Opened(u64),
        Record(u64, Vec<u8>),
        Closed(u64),
    }
    pub struct Driver {
        pub link: Link,
        started: std::time::Instant,
        events: mpsc::UnboundedSender<Event>,
        inbox: mpsc::UnboundedReceiver<Event>,
        writers: HashMap<u64, mpsc::UnboundedSender<Vec<u8>>>,
        pub delivered: Vec<Value>,
        pub status: Option<LinkStatus>,
        pub connected: bool,
        pub ended: Option<LinkEnd>,
        wait_ms: u64,
    }
    impl Driver {
        pub fn open(invited: Route, invitation: Invitation) -> Self {
            let (link, effects) = Link::open(invited, invitation, None, "test", 0);
            let (events, inbox) = mpsc::unbounded_channel();
            let mut driver = Self {
                link,
                started: std::time::Instant::now(),
                events,
                inbox,
                writers: HashMap::new(),
                delivered: vec![],
                status: None,
                connected: false,
                ended: None,
                wait_ms: 0,
            };
            driver.apply(effects);
            driver
        }
        fn now(&self) -> u64 {
            self.started.elapsed().as_millis() as u64
        }
        fn apply(&mut self, effects: Effects) {
            for (id, url) in effects.open {
                let (writer, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
                self.writers.insert(id, writer);
                let events = self.events.clone();
                tokio::spawn(async move {
                    let Ok((socket, _)) = tokio_tungstenite::connect_async(url).await else {
                        let _ = events.send(Event::Closed(id));
                        return;
                    };
                    let _ = events.send(Event::Opened(id));
                    let (mut sink, mut stream) = socket.split();
                    loop {
                        tokio::select! {
                            write = outgoing.recv() => match write {
                                Some(bytes) => if sink.send(Message::Binary(bytes)).await.is_err() { break },
                                None => return,
                            },
                            read = stream.next() => match read {
                                Some(Ok(Message::Binary(bytes))) => { let _ = events.send(Event::Record(id, bytes)); }
                                Some(Ok(_)) => {}
                                _ => break,
                            },
                        }
                    }
                    let _ = events.send(Event::Closed(id));
                });
            }
            for id in effects.close {
                self.writers.remove(&id);
            }
            for (id, records) in effects.write {
                if let Some(writer) = self.writers.get(&id) {
                    for record in records {
                        let _ = writer.send(record);
                    }
                }
            }
            for message in effects.deliver {
                self.delivered
                    .push(serde_json::from_slice(&message).unwrap());
            }
            if effects.status.is_some() {
                self.status = effects.status;
            }
            self.connected |= effects.connected;
            self.ended = self.ended.or(effects.ended);
            self.wait_ms = effects.wait_ms;
        }
        pub fn send(&mut self, value: Value) {
            let now = self.now();
            let (accepted, effects) = self.link.send(value.to_string().as_bytes(), now);
            assert!(accepted);
            self.apply(effects);
        }
        /// Run events until `done` holds or the deadline passes.
        pub async fn until(&mut self, mut done: impl FnMut(&Self) -> bool) {
            within(async {
                while !done(self) {
                    let wait = Duration::from_millis(self.wait_ms.max(1));
                    let event = tokio::time::timeout(wait, self.inbox.recv()).await;
                    let now = self.now();
                    let effects = match event {
                        Ok(Some(Event::Opened(id))) => self.link.socket_opened(id, now),
                        Ok(Some(Event::Record(id, bytes))) => self.link.record(id, bytes, now),
                        Ok(Some(Event::Closed(id))) => {
                            if self.writers.remove(&id).is_none() {
                                continue;
                            }
                            self.link.socket_closed(id, now)
                        }
                        _ => self.link.tick(now),
                    };
                    self.apply(effects);
                }
            })
            .await;
        }
    }
    pub fn request(id: u64, method: &str, params: Value) -> Value {
        json!({"id":id,"method":method,"params":params})
    }
    pub fn reply(driver: &Driver, id: u64) -> Option<Value> {
        driver.delivered.iter().find(|f| f["id"] == id).cloned()
    }
    pub fn connected_on(driver: &Driver, state: LinkState, kind: RouteKind) -> bool {
        driver.status.as_ref().is_some_and(|s| {
            s.state == state && s.route.as_ref().is_some_and(|route| route.kind == kind)
        })
    }
}

#[tokio::test]
async fn the_mobile_link_works_against_a_real_host() {
    use sans_io::*;
    use zuse_remote::link::LinkState;
    let host = Host::new().await;
    let listener = host.listen().await;
    let mut driver = Driver::open(
        Route {
            kind: RouteKind::Lan,
            origin: listener.origin.clone(),
        },
        host.invitation.clone(),
    );
    driver.until(|d| d.connected).await;
    assert!(connected_on(&driver, LinkState::Connected, RouteKind::Lan));
    let large = json!({"text":"streamed transcript text ".repeat(4_000)});
    driver.send(request(1, "Echo", large.clone()));
    driver.until(|d| reply(d, 1).is_some()).await;
    assert_eq!(reply(&driver, 1).unwrap()["ok"], large);
    listener.stop().await;
}

#[tokio::test]
async fn the_mobile_link_resumes_on_another_route_without_gaps() {
    use sans_io::*;
    use zuse_remote::link::LinkState;
    let host = Host::new().await;
    let first = host.listen().await;
    let second = host.listen().await;
    host.routes.send_replace(vec![Route {
        kind: RouteKind::Tailnet,
        origin: second.origin.clone(),
    }]);
    let mut driver = Driver::open(
        Route {
            kind: RouteKind::Lan,
            origin: first.origin.clone(),
        },
        host.invitation.clone(),
    );
    driver.until(|d| d.connected).await;
    driver.send(request(1, "Ticks", json!({})));
    for index in 0..30 {
        host.ticks.send(json!({"n":index})).await.unwrap();
    }
    driver
        .until(|d| {
            d.delivered
                .iter()
                .filter(|f| f["item"]["n"].is_i64())
                .count()
                == 30
        })
        .await;
    first.stop().await;
    for index in 30..60 {
        host.ticks.send(json!({"n":index})).await.unwrap();
    }
    driver.send(request(2, "Count", json!({"after":"move"})));
    driver.until(|d| reply(d, 2).is_some()).await;
    driver
        .until(|d| {
            d.delivered
                .iter()
                .filter(|f| f["item"]["n"].is_i64())
                .count()
                == 60
        })
        .await;
    let seen: Vec<i64> = driver
        .delivered
        .iter()
        .filter_map(|f| f["item"]["n"].as_i64())
        .collect();
    assert_eq!(seen, (0..60).collect::<Vec<_>>());
    assert_eq!(host.executions.load(Ordering::SeqCst), 1);
    assert!(connected_on(
        &driver,
        LinkState::Connected,
        RouteKind::Tailnet
    ));
    second.stop().await;
}

#[tokio::test]
async fn the_mobile_link_upgrades_itself_to_a_better_route() {
    use sans_io::*;
    use zuse_remote::link::LinkState;
    let host = Host::new().await;
    let first = host.listen().await;
    let better = host.listen().await;
    host.routes.send_replace(vec![Route {
        kind: RouteKind::Lan,
        origin: better.origin.clone(),
    }]);
    let mut driver = Driver::open(
        Route {
            kind: RouteKind::Relay,
            origin: first.origin.clone(),
        },
        host.invitation.clone(),
    );
    driver.until(|d| d.connected).await;
    driver.send(request(1, "Ticks", json!({})));
    host.ticks.send(json!({"n":0})).await.unwrap();
    driver
        .until(|d| connected_on(d, LinkState::Connected, RouteKind::Lan))
        .await;
    host.ticks.send(json!({"n":1})).await.unwrap();
    driver
        .until(|d| {
            d.delivered
                .iter()
                .filter(|f| f["item"]["n"].is_i64())
                .count()
                == 2
        })
        .await;
    assert_eq!(host.sessions.len(), 1);
    first.stop().await;
    better.stop().await;
}
