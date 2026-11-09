use super::HostToken;
use crate::BrowserServer;
use axum::extract::ws::Message;
use futures::{SinkExt, StreamExt};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{
    sync::{Notify, Semaphore},
    task::JoinSet,
};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, protocol::WebSocketConfig};
use zuse_remote::{MAX_RECORD_BYTES, PairId, authority::Credentials};
/// Socket counts describe broker transport, not encrypted application health.
#[derive(Clone, Default)]
pub struct HostStatus(Arc<Counts>);
#[derive(Default)]
struct Counts {
    sockets: std::sync::atomic::AtomicUsize,
    sessions: std::sync::atomic::AtomicUsize,
}
impl HostStatus {
    pub fn sockets(&self) -> usize {
        self.0.sockets.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn sessions(&self) -> usize {
        self.0.sessions.load(std::sync::atomic::Ordering::Acquire)
    }
    fn track(&self, session: bool) -> CountGuard {
        let counter = if session {
            &self.0.sessions
        } else {
            &self.0.sockets
        };
        counter.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        CountGuard(self.clone(), session)
    }
}
struct CountGuard(HostStatus, bool);
impl Drop for CountGuard {
    fn drop(&mut self) {
        let counter = if self.1 {
            &self.0.0.sessions
        } else {
            &self.0.0.sockets
        };
        counter.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
const STANDBY_PER_PAIR: usize = 4;
// 32 saved grants × four standby sockets plus the profile's 64 active sessions.
const MAX_WORKERS: usize = 192;
struct Standby {
    count: Arc<std::sync::atomic::AtomicUsize>,
    changed: Arc<Notify>,
}
impl Standby {
    fn new(count: Arc<std::sync::atomic::AtomicUsize>, changed: Arc<Notify>) -> Self {
        count.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self { count, changed }
    }
}
impl Drop for Standby {
    fn drop(&mut self) {
        self.count.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        self.changed.notify_one();
    }
}
async fn pool(
    server: BrowserServer,
    authority: zuse_remote::authority::Authority,
    device: String,
    credentials: Arc<Credentials>,
    origin: String,
    token: HostToken,
    metrics: HostStatus,
    budget: Arc<Semaphore>,
) {
    let standby = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let changed = Arc::new(Notify::new());
    let mut workers = JoinSet::new();
    loop {
        tokio::select! { biased;
            _ = server.shutdown.cancelled() => break,
            _ = credentials.revoked() => break,
            _ = workers.join_next(), if !workers.is_empty() => {},
            _ = changed.notified() => {},
            permit = budget.clone().acquire_owned(), if standby.load(std::sync::atomic::Ordering::Acquire) < STANDBY_PER_PAIR => {
                let Ok(permit) = permit else { break; };
                let standby = Standby::new(standby.clone(),changed.clone());
                let (server,authority,device,credentials,origin,token,metrics) = (server.clone(),authority.clone(),device.clone(),credentials.clone(),origin.clone(),token.clone(),metrics.clone());
                workers.spawn(async move {
                    let _permit = permit;
                    worker(server,authority,device,credentials,origin,token,metrics,standby).await;
                });
            }
        }
    }
    workers.abort_all();
    while workers.join_next().await.is_some() {}
}
/// Maintain outbound rendezvous sockets for saved grants. RPC and Noise remain
/// authorized by the execution host, including revocation and profile scope.
/// This future owns all workers; cancellation or drop cannot detach them.
pub async fn serve_host(
    server: BrowserServer,
    origin: &str,
    token: HostToken,
) -> anyhow::Result<()> {
    serve_host_with_status(server, origin, token, HostStatus::default()).await
}
pub async fn serve_host_with_status(
    server: BrowserServer,
    origin: &str,
    token: HostToken,
    metrics: HostStatus,
) -> anyhow::Result<()> {
    let origin = zuse_remote::normalized_origin(origin)?;
    let shutdown = server.shutdown.child_token();
    let _drop_guard = shutdown.clone().drop_guard();
    let server = server.with_shutdown_token(shutdown);
    let budget = Arc::new(Semaphore::new(MAX_WORKERS));
    let (authority, device) = server
        .pairings
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Pairing authority is unavailable"))?;
    let mut running = HashSet::new();
    let mut tasks = JoinSet::new();
    let mut scan = tokio::time::interval(Duration::from_secs(1));
    scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;
            _=server.shutdown.cancelled()=>break,
            done=tasks.join_next(),if !tasks.is_empty()=>{if let Some(Ok(id))=done{running.remove(&id);}},
            _=scan.tick()=>{
                let Ok(peers)=authority.list()else{break;};
                for peer in peers {
                    let Ok(id)=PairId::parse(&peer.id)else{continue;};
                    // Refresh expiry even for existing workers, cancelling stale invitations.
                    let Ok(credentials)=authority.credentials(&id)else{continue;};
                    if !running.insert(peer.id.clone()){continue;}
                    let server=server.clone();let token=token.clone();let origin=origin.clone();let authority=authority.clone();let device=device.clone();let metrics=metrics.clone();let budget=budget.clone();
                    tasks.spawn(async move{
                        let credentials=Arc::new(credentials);
                        pool(server,authority,device,credentials,origin,token,metrics,budget).await;
                        peer.id
                    });
                }
            }
        }
    }
    server.shutdown.cancel();
    // Normal shutdown lets each pool drain its own tracked workers. The drop
    // guard and nested JoinSets also revoke ownership if this future is aborted.
    let drained = tokio::time::timeout(Duration::from_secs(5), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    Ok(())
}
async fn worker(
    server: BrowserServer,
    authority: zuse_remote::authority::Authority,
    device: String,
    credentials: Arc<Credentials>,
    origin: String,
    token: HostToken,
    metrics: HostStatus,
    standby: Standby,
) {
    let mut standby = Some(standby);
    let scheme = if origin.starts_with("https:") {
        "wss"
    } else {
        "ws"
    };
    let url = format!(
        "{scheme}{}/api/paired-host/{}",
        &origin[origin.find(':').unwrap()..],
        credentials.id().as_str()
    );
    let mut delay = Duration::from_millis(250);
    loop {
        if !credentials.is_live() || server.shutdown.is_cancelled() {
            return;
        }
        let attempt = async {
            let mut request = url.clone().into_client_request().map_err(|_| ())?;
            request.headers_mut().insert(
                tungstenite::http::header::AUTHORIZATION,
                token.header().parse().map_err(|_| ())?,
            );
            let config = WebSocketConfig {
                max_message_size: Some(MAX_RECORD_BYTES),
                max_frame_size: Some(MAX_RECORD_BYTES),
                ..Default::default()
            };
            let mut socket = tokio::time::timeout(
                Duration::from_secs(10),
                zuse_sync::dial::connect_ws_with_config(request, Some(config)),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?
            .socket;
            let _socket_count = metrics.track(false);
            let mut ping = tokio::time::interval(Duration::from_secs(15));
            let mut seen = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    _=ping.tick()=>{
                        if seen.elapsed()>Duration::from_secs(45){return Err(());}
                        tokio::time::timeout(Duration::from_secs(5),socket.send(tungstenite::Message::Ping(vec![]))).await.map_err(|_|())?.map_err(|_|())?;
                    },
                    frame=socket.next()=>{seen=tokio::time::Instant::now();match frame {
                        Some(Ok(tungstenite::Message::Text(text)))if text=="connected"=>break,
                        Some(Ok(tungstenite::Message::Ping(data)))=>{tokio::time::timeout(Duration::from_secs(5),socket.send(tungstenite::Message::Pong(data))).await.map_err(|_|())?.map_err(|_|())?;},
                        Some(Ok(tungstenite::Message::Pong(_)))=>{},_=>return Err(())
                    }}
                }
            }
            // Replenish the standby pool as soon as the broker assigns a
            // client. This worker owns one session and never re-registers.
            drop(standby.take());
            let _permit = server
                .paired_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| ())?;
            let _session_count = metrics.track(true);
            // Translate framing only; the same host session performs all crypto,
            // proof confirmation and canonical authorization as direct access.
            let socket = socket
                .with(|message: Message| {
                    futures::future::ready(Ok::<_, tungstenite::Error>(match message {
                        Message::Binary(data) => tungstenite::Message::Binary(data.to_vec()),
                        Message::Ping(data) => tungstenite::Message::Ping(data.to_vec()),
                        Message::Pong(data) => tungstenite::Message::Pong(data.to_vec()),
                        _ => tungstenite::Message::Close(None),
                    }))
                })
                .map(|frame| {
                    frame.map(|message| match message {
                        tungstenite::Message::Binary(data) => Message::Binary(data.into()),
                        tungstenite::Message::Ping(data) => Message::Ping(data.into()),
                        tungstenite::Message::Pong(data) => Message::Pong(data.into()),
                        _ => Message::Close(None),
                    })
                });
            crate::paired::session(
                server.clone(),
                authority.clone(),
                device.clone(),
                credentials.clone(),
                socket,
            )
            .await;
            Ok::<_, ()>(())
        };
        let success = tokio::select! {biased;
            _=server.shutdown.cancelled()=>return,
            _=credentials.revoked()=>return,
            result=attempt=>result.is_ok(),
        };
        if standby.is_none() {
            return;
        }
        delay = if success {
            Duration::from_millis(250)
        } else {
            (delay * 2).min(Duration::from_secs(10))
        };
        tokio::select! {_=server.shutdown.cancelled()=>return,_=credentials.revoked()=>return,_=tokio::time::sleep(delay)=>{}}
    }
}
