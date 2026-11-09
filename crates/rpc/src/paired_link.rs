//! One stable channel to a paired host over whichever route is fastest.
//!
//! Connecting races the known routes. Once connected, the link keeps measuring
//! the others and moves the session when one is clearly better, and it replaces
//! a lost socket the same way. Callers above see one uninterrupted [`Channel`]:
//! a move resumes from delivery counts instead of replaying requests, and only
//! a session the host no longer holds closes the channel.
use crate::{RpcError, ServerFrame, paired, reconnect::Channel};
use futures::{
    SinkExt, StreamExt,
    stream::{FuturesUnordered, SplitSink, SplitStream},
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::Instant,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;
use zuse_remote::{
    Invitation, MAX_RECORD_BYTES, PairedTarget, Route, RouteKind, RouteTable, SecureChannel,
    heartbeat::{Action, Heartbeat},
    routes::{Decision, SavedRoutes, classify_origin},
    session::{self, AttachReply, AttachRequest, Control, Resume},
};
use zuse_sync::socket::ProgressIo;

/// Later routes join the race only if the earlier ones have not answered.
const RACE_STAGGER: Duration = Duration::from_millis(200);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TICK: Duration = Duration::from_secs(2);
const PATH_CHANGE_CHECK_MS: u64 = 3_000;
const MAX_REATTACH_BACKOFF: Duration = Duration::from_secs(2);

type Ws = WebSocketStream<MaybeTlsStream<ProgressIo<tokio::net::TcpStream>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkState {
    Connected,
    /// The socket was lost and the session is being moved to a new one.
    Reattaching,
}

/// What the interface shows about the active connection. No secrets.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkStatus {
    pub state: LinkState,
    pub route: Route,
    pub rtt_ms: Option<u32>,
    pub resumable: bool,
    pub compressed: bool,
}

struct Shared {
    invitation: Invitation,
    table: Mutex<RouteTable>,
    store: Option<PathBuf>,
    status: watch::Sender<Option<LinkStatus>>,
    /// Only one link per host measures routes; auxiliary links follow it.
    supervised: AtomicBool,
    epoch: Instant,
}
impl Shared {
    fn table(&self) -> std::sync::MutexGuard<'_, RouteTable> {
        self.table.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }
    fn persist(&self) {
        let Some(path) = &self.store else { return };
        let saved = self.table().saved();
        let Ok(bytes) = serde_json::to_vec(&saved) else {
            return;
        };
        if let Err(error) = write_private(path, &bytes) {
            tracing::debug!(%error, "paired routes could not be saved");
        }
    }
    fn learn(&self, reported: Vec<Route>) {
        if self.table().learn(reported) {
            self.persist();
        }
    }
    fn refresh_network(&self) {
        let network = zuse_sync::net_path::network_id();
        self.table().network_changed(&network);
    }
}
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)
}

/// Every connection to one paired host shares this route memory.
#[derive(Clone)]
pub struct PairedHost(Arc<Shared>);
impl PairedHost {
    /// `directory` keeps learned routes across launches; they hold no secrets.
    pub fn new(target: &PairedTarget, directory: Option<&Path>) -> Self {
        let invited = Route {
            kind: classify_origin(target.origin()),
            origin: target.origin().to_owned(),
        };
        let mut table = RouteTable::new(invited);
        let invitation = target.invitation().clone();
        let store = directory.map(|d| d.join(format!("{}.json", invitation.id.as_str())));
        if let Some(saved) = store
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<SavedRoutes>(&bytes).ok())
        {
            table.restore(saved);
        }
        Self::from_parts(table, invitation, store)
    }
    fn from_parts(table: RouteTable, invitation: Invitation, store: Option<PathBuf>) -> Self {
        Self(Arc::new(Shared {
            invitation,
            table: Mutex::new(table),
            store,
            status: watch::Sender::new(None),
            supervised: AtomicBool::new(false),
            epoch: Instant::now(),
        }))
    }
    /// As [`Self::new`], for an address whose kind is known better than its
    /// text shows, such as a relay running on a private network.
    pub fn with_route(invited: Route, invitation: Invitation, directory: Option<&Path>) -> Self {
        let store = directory.map(|d| d.join(format!("{}.json", invitation.id.as_str())));
        let table = RouteTable::new(invited);
        Self::from_parts(table, invitation, store)
    }
    pub fn status(&self) -> watch::Receiver<Option<LinkStatus>> {
        self.0.status.subscribe()
    }
    pub fn routes(&self) -> Vec<Route> {
        self.0.table().routes()
    }
    /// Add routes reported out of band, such as over the account relay.
    pub fn learn(&self, routes: Vec<Route>) {
        self.0.learn(routes);
    }
    /// Race the routes, attach, and return a channel that survives route moves.
    pub async fn connect(&self, cancel: CancellationToken) -> Result<Channel, RpcError> {
        zuse_sync::net_path::spawn_path_monitor();
        self.0.refresh_network();
        tokio::select! {biased;
            _ = cancel.cancelled() => Err(RpcError::Closed),
            value = open_link(self.0.clone(), cancel.clone()) => value,
        }
    }
}

enum DialError {
    Authentication,
    Transport,
}

/// Noise is complete, so whoever answered holds the pairing secret.
struct Dialed {
    route: Route,
    socket: Ws,
    channel: SecureChannel,
}

async fn dial(route: Route, invitation: Invitation) -> Result<Dialed, DialError> {
    let authenticate = async {
        let target = PairedTarget::for_route(&route.origin, invitation.clone())
            .map_err(|_| DialError::Transport)?;
        let config = WebSocketConfig {
            max_message_size: Some(MAX_RECORD_BYTES),
            max_frame_size: Some(MAX_RECORD_BYTES),
            ..Default::default()
        };
        let mut socket = zuse_sync::dial::connect_ws_with_config(target.socket_url(), Some(config))
            .await
            .map_err(|_| DialError::Transport)?
            .socket;
        let (handshake, first) = zuse_remote::initiate(&invitation.id, &invitation.secret)
            .map_err(|_| DialError::Transport)?;
        socket
            .send(Message::Binary(first))
            .await
            .map_err(|_| DialError::Transport)?;
        loop {
            match socket.next().await {
                Some(Ok(Message::Binary(reply))) => {
                    let channel = handshake
                        .finish(&reply)
                        .map_err(|_| DialError::Authentication)?;
                    return Ok(Dialed {
                        route,
                        socket,
                        channel,
                    });
                }
                Some(Ok(Message::Ping(payload))) => socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(|_| DialError::Transport)?,
                Some(Ok(Message::Pong(_))) => {}
                _ => return Err(DialError::Transport),
            }
        }
    };
    tokio::time::timeout(DIAL_TIMEOUT, authenticate)
        .await
        .map_err(|_| DialError::Transport)?
}

/// First route through Noise wins. Sessions are opened only on the winner.
async fn race(shared: &Arc<Shared>, routes: Vec<Route>) -> Result<Dialed, RpcError> {
    let mut queue = VecDeque::from(routes);
    let mut pending = FuturesUnordered::new();
    let mut authentication_failed = false;
    let start = |route: Route, pending: &mut FuturesUnordered<_>| {
        let attempt_route = route.clone();
        let invitation = shared.invitation.clone();
        pending.push(async move { (attempt_route, dial(route, invitation).await) });
    };
    let Some(first) = queue.pop_front() else {
        return Err(RpcError::Transport("No route to the paired engine".into()));
    };
    start(first, &mut pending);
    let stagger = tokio::time::sleep(RACE_STAGGER);
    tokio::pin!(stagger);
    loop {
        tokio::select! {
            finished = pending.next(), if !pending.is_empty() => {
                let Some((route, result)) = finished else { continue };
                match result {
                    Ok(dialed) => return Ok(dialed),
                    Err(error) => {
                        authentication_failed |= matches!(error, DialError::Authentication);
                        shared.table().record_failure(&route, shared.now());
                        if let Some(next) = queue.pop_front() {
                            start(next, &mut pending);
                            stagger.as_mut().reset(Instant::now() + RACE_STAGGER);
                        } else if pending.is_empty() {
                            return Err(if authentication_failed {
                                RpcError::Failed("Paired engine authentication failed. Open a new invitation.".into())
                            } else {
                                RpcError::Transport("Could not reach paired engine".into())
                            });
                        }
                    }
                }
            },
            _ = stagger.as_mut(), if !queue.is_empty() => {
                if let Some(next) = queue.pop_front() {
                    start(next, &mut pending);
                }
                stagger.as_mut().reset(Instant::now() + RACE_STAGGER);
            },
        }
    }
}

struct Attached {
    route: Route,
    writer: SplitSink<Ws, Message>,
    reader: SplitStream<Ws>,
    channel: SecureChannel,
    heartbeat: Heartbeat,
    clock: Instant,
}
impl Attached {
    fn elapsed(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }
}

fn attach_frame(request: &AttachRequest) -> Result<String, ()> {
    serde_json::to_string(&serde_json::json!({
        "id": session::ATTACH_ID,
        "method": session::ATTACH_METHOD,
        "params": request,
    }))
    .map_err(|_| ())
}

fn tuned(route: &Route, mut heartbeat: Heartbeat) -> Heartbeat {
    // A dead LAN socket should be replaced at once. A tailnet on a relayed
    // path or a mobile link can stall this long while perfectly alive.
    match route.kind {
        RouteKind::Loopback | RouteKind::Lan => heartbeat.set_limits(10_000, 5_000),
        _ => heartbeat.set_limits(15_000, 15_000),
    }
    heartbeat
}

enum Attachment {
    Session(AttachReply),
    /// The host predates sessions; this socket works as a plain connection.
    Legacy,
}

/// Heartbeat and attach share one flight, so identity costs no extra round trip.
async fn confirm_and_attach(
    dialed: Dialed,
    request: &AttachRequest,
) -> Result<(Attached, Attachment), RpcError> {
    let failed = || RpcError::Transport("Paired host did not confirm the connection".into());
    let Dialed {
        route,
        mut socket,
        mut channel,
    } = dialed;
    let clock = Instant::now();
    let mut heartbeat = Heartbeat::new();
    let Ok(Action::Send(probe)) = heartbeat.poll(0) else {
        return Err(failed());
    };
    let attach = attach_frame(request).map_err(|_| failed())?;
    let exchange = async {
        paired::send_records(&mut socket, &mut channel, &probe).await?;
        paired::send_records(&mut socket, &mut channel, attach.as_bytes()).await?;
        let mut confirmed = false;
        loop {
            let payload = read_payload(&mut socket, &mut channel).await?;
            let now = clock.elapsed().as_millis() as u64;
            if heartbeat.consume(&payload, now).map_err(|_| ())? {
                confirmed = true;
                continue;
            }
            let attachment = parse_attach(&payload)?;
            return if confirmed { Ok(attachment) } else { Err(()) };
        }
    };
    let attachment = tokio::time::timeout(ATTACH_TIMEOUT, exchange)
        .await
        .map_err(|_| failed())?
        .map_err(|()| failed())?;
    let (writer, reader) = socket.split();
    Ok((
        Attached {
            heartbeat: tuned(&route, heartbeat),
            route,
            writer,
            reader,
            channel,
            clock,
        },
        attachment,
    ))
}

async fn read_payload(socket: &mut Ws, channel: &mut SecureChannel) -> Result<Vec<u8>, ()> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Binary(record))) => {
                if let Some(payload) = channel.open(&record).map_err(|_| ())? {
                    return Ok(payload);
                }
            }
            Some(Ok(Message::Ping(payload))) => {
                paired::send(socket, Message::Pong(payload)).await?
            }
            Some(Ok(Message::Pong(_))) => {}
            _ => return Err(()),
        }
    }
}

fn parse_attach(payload: &[u8]) -> Result<Attachment, ()> {
    let frame: ServerFrame = serde_json::from_slice(payload).map_err(|_| ())?;
    if frame.id != session::ATTACH_ID {
        return Err(());
    }
    if frame.err.is_some() {
        return Ok(Attachment::Legacy);
    }
    let reply = serde_json::from_value(frame.ok.ok_or(())?).map_err(|_| ())?;
    Ok(Attachment::Session(reply))
}

async fn open_link(shared: Arc<Shared>, cancel: CancellationToken) -> Result<Channel, RpcError> {
    let order = shared.table().dial_order(false);
    let dialed = race(&shared, order).await?;
    let request = AttachRequest {
        session: None,
        received: 0,
        caps: vec![session::CAP_LZ4.into()],
    };
    let (attached, attachment) = confirm_and_attach(dialed, &request).await?;
    let (session_id, compress, engine_info) = match attachment {
        Attachment::Session(reply) => {
            shared.learn(reply.routes);
            (
                Some(reply.session),
                reply.caps.iter().any(|cap| cap == session::CAP_LZ4),
                reply.engine_info,
            )
        }
        Attachment::Legacy => (None, false, None),
    };
    shared
        .table()
        .record_connected(&attached.route, shared.now());
    shared.persist();
    if let Some(rtt) = attached.heartbeat.last_rtt_ms() {
        shared.table().record_rtt(&attached.route, rtt);
    }
    // A probe-driven move needs a resumable session. Older hosts keep the
    // route that won the race until their connection ends.
    let supervises = session_id.is_some() && !shared.supervised.swap(true, Ordering::AcqRel);
    let (out_tx, out_rx) = mpsc::channel::<String>(16);
    let (in_tx, in_rx) = mpsc::channel::<String>(16);
    let closed = CancellationToken::new();
    let link = Link {
        shared,
        cancel,
        out_rx,
        in_tx,
        session: session_id,
        compress,
        supervises,
        resume: Resume::default(),
        probes: JoinSet::new(),
    };
    link.publish(&attached, LinkState::Connected);
    tokio::spawn(link.run(attached, closed.clone()));
    Ok(Channel {
        outgoing: out_tx,
        incoming: in_rx,
        closed,
        engine_info,
    })
}

/// A route proven by Noise and a heartbeat, with the measured round trip.
type ProbeResult = Result<(Dialed, Heartbeat, u32), ()>;

enum Exit {
    Stop,
    Lost,
    Switch(Box<Attached>),
}

struct Link {
    shared: Arc<Shared>,
    cancel: CancellationToken,
    out_rx: mpsc::Receiver<String>,
    in_tx: mpsc::Sender<String>,
    session: Option<String>,
    compress: bool,
    supervises: bool,
    resume: Resume,
    probes: JoinSet<(Route, ProbeResult)>,
}
impl Link {
    fn publish(&self, attached: &Attached, state: LinkState) {
        let status = LinkStatus {
            state,
            route: attached.route.clone(),
            rtt_ms: self.shared.table().rtt(&attached.route),
            resumable: self.session.is_some(),
            compressed: self.compress,
        };
        self.shared.status.send_if_modified(|current| {
            let changed = current.as_ref() != Some(&status);
            *current = Some(status);
            changed
        });
    }

    async fn run(mut self, first: Attached, closed: CancellationToken) {
        let _closed = closed.drop_guard();
        let mut current = first;
        loop {
            self.publish(&current, LinkState::Connected);
            match self.exchange(&mut current).await {
                Exit::Stop => break,
                Exit::Switch(next) => current = *next,
                Exit::Lost => {
                    current.channel.close();
                    self.publish(&current, LinkState::Reattaching);
                    let Some(next) = self.reattach().await else {
                        break;
                    };
                    current = next;
                }
            }
        }
        if self.supervises {
            self.shared.supervised.store(false, Ordering::Release);
        }
        self.shared.status.send_replace(None);
    }

    async fn write_control(attached: &mut Attached, frame: &[u8]) -> Result<(), ()> {
        paired::send_records(&mut attached.writer, &mut attached.channel, frame).await
    }

    fn spawn_due_probes(&mut self, current: &Route) {
        if !self.supervises || !self.probes.is_empty() {
            return;
        }
        let due = self
            .shared
            .table()
            .probes_due(current, self.shared.now(), false);
        for route in due {
            let invitation = self.shared.invitation.clone();
            self.probes.spawn(async move {
                let result = probe(route.clone(), invitation).await;
                (route, result)
            });
        }
    }

    /// Serve one socket until it fails, the caller stops, or a better route wins.
    async fn exchange(&mut self, attached: &mut Attached) -> Exit {
        let mut ack_tick = tokio::time::interval(Duration::from_millis(session::ACK_INTERVAL_MS));
        let mut probe_tick = tokio::time::interval(PROBE_TICK);
        let mut network = zuse_sync::wake::subscribe_network_changed();
        let mut resumed = zuse_sync::wake::subscribe();
        loop {
            let wait = match attached.heartbeat.poll(attached.elapsed()) {
                Ok(Action::Send(probe)) => {
                    if Self::write_control(attached, &probe).await.is_err() {
                        return Exit::Lost;
                    }
                    continue;
                }
                Ok(Action::Wait(ms)) => ms,
                Ok(Action::Expired) | Err(_) => return Exit::Lost,
            };
            let can_send = self.resume.can_send();
            let resumable = self.session.is_some();
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Exit::Stop,
                _ = self.in_tx.closed() => return Exit::Stop,
                _ = tokio::time::sleep(Duration::from_millis(wait)) => {},
                _ = ack_tick.tick(), if resumable => {
                    if let Some(ack) = self.resume.pending_ack()
                        && Self::write_control(attached, &ack).await.is_err()
                    {
                        return Exit::Lost;
                    }
                },
                _ = probe_tick.tick(), if self.supervises && attached.route.kind > RouteKind::Lan => {
                    self.spawn_due_probes(&attached.route);
                },
                _ = network.recv() => self.on_path_change(attached),
                _ = resumed.recv() => self.on_path_change(attached),
                finished = self.probes.join_next(), if !self.probes.is_empty() => {
                    let Some(Ok((route, result))) = finished else { continue };
                    if let Some(exit) = self.on_probe(attached, route, result).await {
                        return exit;
                    }
                },
                outgoing = self.out_rx.recv(), if can_send => {
                    let Some(outgoing) = outgoing else { return Exit::Stop };
                    let wire = session::encode_message(outgoing.as_bytes(), self.compress);
                    if resumable {
                        // Kept before writing: a failed write is resent after reattach.
                        self.resume.record_sent(wire.clone());
                    }
                    if paired::send_records(&mut attached.writer, &mut attached.channel, &wire).await.is_err() {
                        return Exit::Lost;
                    }
                },
                incoming = attached.reader.next() => {
                    if let Some(exit) = self.on_incoming(attached, incoming).await {
                        return exit;
                    }
                },
            }
        }
    }

    fn on_path_change(&mut self, attached: &mut Attached) {
        self.shared.refresh_network();
        let now = attached.elapsed();
        attached.heartbeat.expedite(now, PATH_CHANGE_CHECK_MS);
        self.spawn_due_probes(&attached.route);
    }

    async fn on_incoming(
        &mut self,
        attached: &mut Attached,
        incoming: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    ) -> Option<Exit> {
        let record = match incoming {
            Some(Ok(Message::Binary(record))) => record,
            Some(Ok(Message::Ping(payload))) => {
                return paired::send(&mut attached.writer, Message::Pong(payload))
                    .await
                    .err()
                    .map(|()| Exit::Lost);
            }
            Some(Ok(Message::Pong(_))) => return None,
            _ => return Some(Exit::Lost),
        };
        let payload = match attached.channel.open(&record) {
            Ok(Some(payload)) => payload,
            Ok(None) => return None,
            Err(_) => return Some(Exit::Lost),
        };
        match attached.heartbeat.consume(&payload, attached.elapsed()) {
            Ok(true) => {
                if let Some(rtt) = attached.heartbeat.last_rtt_ms() {
                    self.shared.table().record_rtt(&attached.route, rtt);
                    self.publish(attached, LinkState::Connected);
                }
                return None;
            }
            Ok(false) => {}
            Err(_) => return Some(Exit::Lost),
        }
        if self.session.is_some() {
            match session::parse_control(&payload) {
                Ok(Some(Control::Ack(count))) => {
                    return self.resume.acknowledge(count).err().map(|_| Exit::Lost);
                }
                Ok(Some(Control::Routes(routes))) => {
                    self.shared.learn(routes);
                    return None;
                }
                Ok(Some(Control::End)) => return Some(Exit::Stop),
                Ok(None) => {}
                Err(_) => return Some(Exit::Lost),
            }
        }
        let size = payload.len();
        let Ok(decoded) = session::decode_message(payload) else {
            return Some(Exit::Lost);
        };
        let Ok(text) = String::from_utf8(decoded) else {
            return Some(Exit::Lost);
        };
        match tokio::time::timeout(Duration::from_secs(30), self.in_tx.send(text)).await {
            Ok(Ok(())) => {}
            _ => return Some(Exit::Stop),
        }
        if self.session.is_some()
            && let Some(ack) = self.resume.record_received(size)
            && Self::write_control(attached, &ack).await.is_err()
        {
            return Some(Exit::Lost);
        }
        None
    }

    async fn on_probe(
        &mut self,
        attached: &mut Attached,
        route: Route,
        result: ProbeResult,
    ) -> Option<Exit> {
        let now = self.shared.now();
        let Ok((dialed, heartbeat, rtt)) = result else {
            self.shared.table().record_failure(&route, now);
            return None;
        };
        let decision = self
            .shared
            .table()
            .judge_probe(&attached.route, &route, rtt, now);
        match decision {
            Decision::Stay => None,
            Decision::Confirm => {
                // Compare the next probe against a fresh measurement here too.
                let elapsed = attached.elapsed();
                attached.heartbeat.expedite(elapsed, 15_000);
                None
            }
            Decision::Switch => self.take_over(dialed, heartbeat).await,
        }
    }

    /// The old socket is left unread so the delivery count cannot move.
    async fn take_over(&mut self, dialed: Dialed, heartbeat: Heartbeat) -> Option<Exit> {
        let route = dialed.route.clone();
        let now = self.shared.now();
        let attempt = tokio::time::timeout(ATTACH_TIMEOUT, self.attach(dialed, heartbeat)).await;
        match attempt {
            Ok(Ok(Some(next))) => {
                self.shared.table().record_switched(&route, now);
                self.shared.persist();
                tracing::info!(route = ?route.kind, "paired link moved to a faster route");
                Some(Exit::Switch(Box::new(next)))
            }
            Ok(Ok(None)) => Some(Exit::Stop),
            _ => {
                self.shared.table().record_failed_takeover(&route, now);
                None
            }
        }
    }

    /// `Ok(None)` means the host no longer has this session.
    async fn attach(
        &mut self,
        dialed: Dialed,
        heartbeat: Heartbeat,
    ) -> Result<Option<Attached>, ()> {
        let Dialed {
            route,
            mut socket,
            mut channel,
        } = dialed;
        let request = AttachRequest {
            session: self.session.clone(),
            received: self.resume.received(),
            caps: vec![session::CAP_LZ4.into()],
        };
        let attach = attach_frame(&request)?;
        paired::send_records(&mut socket, &mut channel, attach.as_bytes()).await?;
        let payload = read_payload(&mut socket, &mut channel).await?;
        let Attachment::Session(reply) = parse_attach(&payload)? else {
            return Err(());
        };
        if !reply.resumed {
            return Ok(None);
        }
        self.shared.learn(reply.routes);
        let replay = self.resume.rewind(reply.received).map_err(|_| ())?;
        for wire in replay {
            paired::send_records(&mut socket, &mut channel, &wire).await?;
        }
        let (writer, reader) = socket.split();
        Ok(Some(Attached {
            heartbeat: tuned(&route, heartbeat),
            clock: Instant::now(),
            route,
            writer,
            reader,
            channel,
        }))
    }

    /// Move the session to a new socket inside the host's holding window.
    async fn reattach(&mut self) -> Option<Attached> {
        self.session.as_ref()?;
        self.probes.abort_all();
        let deadline = Instant::now() + Duration::from_millis(session::CLIENT_RESUME_WINDOW_MS);
        let mut backoff = Duration::from_millis(100);
        let mut online = zuse_sync::wake::subscribe_online();
        while Instant::now() < deadline {
            let order = self.shared.table().dial_order(false);
            let attempt = async {
                let dialed = race(&self.shared, order).await.map_err(|_| ())?;
                let route = dialed.route.clone();
                let (dialed, heartbeat, _) = confirm(dialed).await?;
                Ok::<_, ()>((route, dialed, heartbeat))
            };
            let raced = tokio::select! {biased;
                _ = self.cancel.cancelled() => return None,
                _ = self.in_tx.closed() => return None,
                raced = tokio::time::timeout(ATTACH_TIMEOUT + DIAL_TIMEOUT, attempt) => raced,
            };
            if let Ok(Ok((route, dialed, heartbeat))) = raced {
                match tokio::time::timeout(ATTACH_TIMEOUT, self.attach(dialed, heartbeat)).await {
                    Ok(Ok(Some(next))) => {
                        self.shared
                            .table()
                            .record_connected(&route, self.shared.now());
                        self.shared.persist();
                        return Some(next);
                    }
                    Ok(Ok(None)) => return None,
                    _ => {}
                }
            }
            tokio::select! {biased;
                _ = self.cancel.cancelled() => return None,
                _ = self.in_tx.closed() => return None,
                _ = online.recv() => {},
                _ = tokio::time::sleep(backoff) => {},
            }
            backoff = (backoff * 2).min(MAX_REATTACH_BACKOFF);
        }
        None
    }
}

/// Finish proving a dialed route: one encrypted round trip through the host.
async fn confirm(mut dialed: Dialed) -> ProbeResult {
    let clock = Instant::now();
    let mut heartbeat = Heartbeat::new();
    let Ok(Action::Send(challenge)) = heartbeat.poll(0) else {
        return Err(());
    };
    let exchange = async {
        paired::send_records(&mut dialed.socket, &mut dialed.channel, &challenge).await?;
        let payload = read_payload(&mut dialed.socket, &mut dialed.channel).await?;
        let now = clock.elapsed().as_millis() as u64;
        if heartbeat.consume(&payload, now).map_err(|_| ())? {
            Ok::<_, ()>(())
        } else {
            Err(())
        }
    };
    tokio::time::timeout(ATTACH_TIMEOUT, exchange)
        .await
        .map_err(|_| ())??;
    let rtt = heartbeat.last_rtt_ms().ok_or(())?;
    Ok((dialed, heartbeat, rtt))
}

async fn probe(route: Route, invitation: Invitation) -> ProbeResult {
    let dialed = dial(route, invitation).await.map_err(|_| ())?;
    confirm(dialed).await
}
