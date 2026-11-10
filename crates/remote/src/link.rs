//! The paired link as a pure state machine, for platforms that own their sockets.
//!
//! The iOS and Android apps open WebSockets with their own networking stacks
//! and feed events in here: socket opened, record received, socket closed, a
//! message to send, time passing. Everything else (racing routes, measuring
//! them, moving the session, resuming after a loss) is decided here and handed
//! back as [`Effects`], so both apps behave identically and it can be tested
//! without a network. Time is elapsed milliseconds supplied by the caller.
use crate::{
    ClientHandshake, Invitation, PairedTarget, Route, RouteKind, RouteTable, SecureChannel,
    heartbeat::{Action, Heartbeat},
    routes::{Decision, SavedRoutes},
    session::{self, AttachReply, AttachRequest, Control, Resume},
};
use std::collections::{BTreeMap, VecDeque};

const RACE_STAGGER_MS: u64 = 200;
const DIAL_TIMEOUT_MS: u64 = 10_000;
const PROBE_TICK_MS: u64 = 2_000;
const PATH_CHANGE_CHECK_MS: u64 = 3_000;
const CONFIRM_CHECK_MS: u64 = 15_000;
const REATTACH_BACKOFF_START_MS: u64 = 100;
const REATTACH_BACKOFF_MAX_MS: u64 = 2_000;
const IDLE_WAIT_MS: u64 = 60_000;
/// Far past anything a healthy session holds: only a stuck peer gets here.
const MAX_BUFFERED_BYTES: usize = session::MAX_UNACKED_BYTES * 4;

pub type SocketId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Connected,
    Reattaching,
}
impl LinkState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Reattaching => "reattaching",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkStatus {
    pub state: LinkState,
    pub route: Option<Route>,
    pub rtt_ms: Option<u32>,
    pub resumable: bool,
    pub compressed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkEnd {
    /// No route could be reached.
    Unreachable,
    /// A route answered without the pairing secret; the grant is not valid.
    Authentication,
    /// The connection or session is gone; requests in flight are uncertain.
    Lost,
    /// The host ended the session, as on revocation.
    EndedByHost,
    Closed,
}

/// What the platform must do next. Empty fields mean nothing to do.
#[derive(Debug, Default)]
pub struct Effects {
    pub open: Vec<(SocketId, String)>,
    pub close: Vec<SocketId>,
    pub write: Vec<(SocketId, Vec<Vec<u8>>)>,
    pub deliver: Vec<Vec<u8>>,
    /// Call [`Link::tick`] after this long, unless another event comes first.
    pub wait_ms: u64,
    pub status: Option<LinkStatus>,
    pub saved: Option<SavedRoutes>,
    pub engine_info: Option<serde_json::Value>,
    /// Set once, when the channel first becomes usable.
    pub connected: bool,
    pub ended: Option<LinkEnd>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Primary,
    Probe,
}
enum Phase {
    Opening(ClientHandshake, Vec<u8>),
    Noise(ClientHandshake),
    Probing,
    Attaching { confirmed: bool, takeover: bool },
    Live,
}
struct Socket {
    route: Route,
    role: Role,
    phase: Option<Phase>,
    channel: Option<SecureChannel>,
    heartbeat: Heartbeat,
    deadline: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Connecting,
    Connected,
    Reattaching {
        deadline: u64,
        next_attempt: u64,
        backoff: u64,
    },
    Ended,
}

pub struct Link {
    invitation: Invitation,
    table: RouteTable,
    resume: Resume,
    session: Option<String>,
    compress: bool,
    mode: Mode,
    sockets: BTreeMap<SocketId, Socket>,
    next_socket: SocketId,
    queue: VecDeque<Route>,
    next_stagger: u64,
    authentication_failed: bool,
    current: Option<SocketId>,
    takeover: Option<SocketId>,
    /// Records from the old socket, unread so its delivery count stays put.
    held: Vec<Vec<u8>>,
    /// Sent during a takeover: resent by the rewind, or flushed if it fails.
    unwritten: Vec<Vec<u8>>,
    old_socket_lost: bool,
    next_ack: u64,
    next_probe: u64,
    published: Option<LinkStatus>,
    effects: Effects,
}

fn tuned(route: &Route, heartbeat: &mut Heartbeat) {
    // A dead LAN socket should be replaced at once. A tailnet on a relayed
    // path or a mobile link can stall this long while perfectly alive.
    match route.kind {
        RouteKind::Loopback | RouteKind::Lan => heartbeat.set_limits(10_000, 5_000),
        _ => heartbeat.set_limits(15_000, 15_000),
    }
}

impl Link {
    /// Start connecting. `invited` is the address in the pairing link.
    pub fn open(
        invited: Route,
        invitation: Invitation,
        saved: Option<SavedRoutes>,
        network: &str,
        now: u64,
    ) -> (Self, Effects) {
        let mut table = RouteTable::new(invited);
        if let Some(saved) = saved {
            table.restore(saved);
        }
        table.network_changed(network);
        let mut link = Self {
            invitation,
            table,
            resume: Resume::default(),
            session: None,
            compress: false,
            mode: Mode::Connecting,
            sockets: BTreeMap::new(),
            next_socket: 1,
            queue: VecDeque::new(),
            next_stagger: 0,
            authentication_failed: false,
            current: None,
            takeover: None,
            held: vec![],
            unwritten: vec![],
            old_socket_lost: false,
            next_ack: 0,
            next_probe: 0,
            published: None,
            effects: Effects::default(),
        };
        link.start_race(now);
        let effects = link.finish(now);
        (link, effects)
    }

    pub fn socket_opened(&mut self, id: SocketId, now: u64) -> Effects {
        if let Some(socket) = self.sockets.get_mut(&id)
            && let Some(Phase::Opening(handshake, first)) = socket.phase.take()
        {
            socket.phase = Some(Phase::Noise(handshake));
            self.effects.write.push((id, vec![first]));
        }
        self.finish(now)
    }

    pub fn record(&mut self, id: SocketId, record: Vec<u8>, now: u64) -> Effects {
        if self.takeover.is_some() && self.current == Some(id) {
            self.held.push(record);
        } else {
            self.receive(id, record, now);
        }
        self.finish(now)
    }

    pub fn socket_closed(&mut self, id: SocketId, now: u64) -> Effects {
        self.drop_failed(id, false, now);
        self.finish(now)
    }

    /// Queue one application message. False means the link cannot take it.
    pub fn send(&mut self, message: &[u8], now: u64) -> (bool, Effects) {
        let accepted = self.send_message(message, now);
        (accepted, self.finish(now))
    }

    pub fn tick(&mut self, now: u64) -> Effects {
        self.finish(now)
    }

    /// The network changed or the app returned to the foreground. Sockets
    /// may have died silently, and a better route may now be reachable.
    pub fn network_changed(&mut self, network: &str, now: u64) -> Effects {
        self.table.network_changed(network);
        self.next_probe = now;
        if let Some(socket) = self.current.and_then(|id| self.sockets.get_mut(&id)) {
            socket.heartbeat.expedite(now, PATH_CHANGE_CHECK_MS);
        }
        self.finish(now)
    }

    pub fn close(&mut self, now: u64) -> Effects {
        self.end(LinkEnd::Closed);
        self.finish(now)
    }

    fn start_race(&mut self, now: u64) {
        self.queue = self.table.dial_order(false).into();
        self.authentication_failed = false;
        if let Some(route) = self.queue.pop_front() {
            self.open_socket(route, Role::Primary, now);
            self.next_stagger = now + RACE_STAGGER_MS;
        }
    }

    fn open_socket(&mut self, route: Route, role: Role, now: u64) {
        let Ok(target) = PairedTarget::for_route(&route.origin, self.invitation.clone()) else {
            return;
        };
        let Ok((handshake, first)) = crate::initiate(&self.invitation.id, &self.invitation.secret)
        else {
            return;
        };
        let id = self.next_socket;
        self.next_socket += 1;
        self.effects.open.push((id, target.socket_url()));
        self.sockets.insert(
            id,
            Socket {
                route,
                role,
                phase: Some(Phase::Opening(handshake, first)),
                channel: None,
                heartbeat: Heartbeat::new(),
                deadline: now + DIAL_TIMEOUT_MS,
            },
        );
    }

    fn seal_to(&mut self, id: SocketId, payload: &[u8]) -> bool {
        let Some(sealed) = self
            .sockets
            .get_mut(&id)
            .and_then(|socket| socket.channel.as_mut())
            .and_then(|channel| channel.seal(payload).ok())
        else {
            return false;
        };
        self.effects.write.push((id, sealed));
        true
    }

    fn receive(&mut self, id: SocketId, record: Vec<u8>, now: u64) {
        let Some(socket) = self.sockets.get_mut(&id) else {
            return;
        };
        match socket.phase.take() {
            Some(Phase::Noise(handshake)) => match handshake.finish(&record) {
                Ok(channel) => self.authenticated(id, channel, now),
                Err(_) => self.drop_failed(id, true, now),
            },
            Some(phase @ (Phase::Probing | Phase::Attaching { .. } | Phase::Live)) => {
                socket.phase = Some(phase);
                let opened = socket.channel.as_mut().map(|c| c.open(&record));
                match opened {
                    Some(Ok(Some(payload))) => self.message(id, payload, now),
                    Some(Ok(None)) => {}
                    _ => self.drop_failed(id, false, now),
                }
            }
            // A record before the socket has opened is a broken transport.
            Some(Phase::Opening(..)) | None => self.drop_failed(id, false, now),
        }
    }

    fn authenticated(&mut self, id: SocketId, channel: SecureChannel, now: u64) {
        let Some(socket) = self.sockets.get_mut(&id) else {
            return;
        };
        socket.channel = Some(channel);
        socket.deadline = now + DIAL_TIMEOUT_MS;
        if socket.role == Role::Probe {
            socket.phase = Some(Phase::Probing);
            self.challenge(id, now);
            return;
        }
        // First route through Noise wins, so only one socket opens a session.
        self.queue.clear();
        let losers: Vec<SocketId> = self
            .sockets
            .iter()
            .filter(|(other, s)| {
                **other != id && s.role == Role::Primary && Some(**other) != self.current
            })
            .map(|(other, _)| *other)
            .collect();
        for loser in losers {
            self.discard(loser);
        }
        if let Some(socket) = self.sockets.get_mut(&id) {
            socket.phase = Some(Phase::Attaching {
                confirmed: false,
                takeover: false,
            });
        }
        // Heartbeat and attach share one flight, saving a round trip.
        self.challenge(id, now);
        self.send_attach(id);
    }

    fn challenge(&mut self, id: SocketId, now: u64) {
        let probe = self
            .sockets
            .get_mut(&id)
            .and_then(|s| s.heartbeat.poll(now).ok());
        match probe {
            Some(Action::Send(frame)) => {
                if !self.seal_to(id, &frame) {
                    self.drop_failed(id, false, now);
                }
            }
            _ => self.drop_failed(id, false, now),
        }
    }

    fn send_attach(&mut self, id: SocketId) {
        let frame = serde_json::json!({
            "id": session::ATTACH_ID,
            "method": session::ATTACH_METHOD,
            "params": AttachRequest {
                session: self.session.clone(),
                received: self.resume.received(),
                caps: vec![session::CAP_LZ4.into()],
            },
        });
        if let Ok(bytes) = serde_json::to_vec(&frame) {
            self.seal_to(id, &bytes);
        }
    }

    fn message(&mut self, id: SocketId, payload: Vec<u8>, now: u64) {
        let Some(socket) = self.sockets.get_mut(&id) else {
            return;
        };
        let consumed = socket.heartbeat.consume(&payload, now);
        let phase = match &mut socket.phase {
            Some(Phase::Attaching { confirmed, .. }) => {
                if matches!(consumed, Ok(true)) {
                    *confirmed = true;
                }
                1
            }
            Some(Phase::Probing) => 2,
            Some(Phase::Live) => 3,
            _ => 0,
        };
        match (phase, consumed) {
            (_, Err(_)) | (0, _) => self.drop_failed(id, false, now),
            (1, Ok(true)) => {}
            (1, Ok(false)) => self.attach_reply(id, &payload, now),
            (2, Ok(true)) => self.probe_measured(id, now),
            (2, Ok(false)) => self.drop_failed(id, false, now),
            (_, Ok(true)) => {
                let route = socket_route(&self.sockets, id);
                let rtt = self
                    .sockets
                    .get(&id)
                    .and_then(|s| s.heartbeat.last_rtt_ms());
                if let (Some(route), Some(rtt)) = (route, rtt) {
                    self.table.record_rtt(&route, rtt);
                }
            }
            (_, Ok(false)) => self.application(id, payload, now),
        }
    }

    fn probe_measured(&mut self, id: SocketId, now: u64) {
        let Some(route) = socket_route(&self.sockets, id) else {
            return;
        };
        let rtt = self
            .sockets
            .get(&id)
            .and_then(|s| s.heartbeat.last_rtt_ms());
        let current = self.current.and_then(|c| socket_route(&self.sockets, c));
        let (Some(rtt), Some(current)) = (rtt, current) else {
            self.discard(id);
            return;
        };
        match self.table.judge_probe(&current, &route, rtt, now) {
            Decision::Switch if self.takeover.is_none() => {
                if let Some(socket) = self.sockets.get_mut(&id) {
                    socket.phase = Some(Phase::Attaching {
                        confirmed: true,
                        takeover: true,
                    });
                    socket.deadline = now + DIAL_TIMEOUT_MS;
                }
                self.takeover = Some(id);
                self.send_attach(id);
            }
            Decision::Confirm => {
                // Compare the next probe against a fresh measurement here too.
                if let Some(socket) = self.current.and_then(|c| self.sockets.get_mut(&c)) {
                    socket.heartbeat.expedite(now, CONFIRM_CHECK_MS);
                }
                self.next_probe = now;
                self.discard(id);
            }
            _ => self.discard(id),
        }
    }

    fn attach_reply(&mut self, id: SocketId, payload: &[u8], now: u64) {
        let Some(socket) = self.sockets.get(&id) else {
            return;
        };
        let Some(Phase::Attaching {
            confirmed,
            takeover,
        }) = socket.phase
        else {
            return;
        };
        let parsed = parse_attach(payload);
        let reply = match parsed {
            Ok(reply) if confirmed => reply,
            _ => {
                self.drop_failed(id, false, now);
                return;
            }
        };
        let asked_to_resume = self.session.is_some();
        let Some(reply) = reply else {
            // An older host answered with an ordinary error.
            if asked_to_resume {
                self.drop_failed(id, false, now);
            } else {
                self.become_current(id, now);
            }
            return;
        };
        if asked_to_resume && !reply.resumed {
            self.end(LinkEnd::Lost);
            return;
        }
        self.learn(reply.routes);
        if asked_to_resume {
            let Ok(replay) = self.resume.rewind(reply.received) else {
                self.end(LinkEnd::Lost);
                return;
            };
            for wire in replay {
                if !self.seal_to(id, &wire) {
                    self.drop_failed(id, false, now);
                    return;
                }
            }
            self.unwritten.clear();
        } else {
            self.session = Some(reply.session);
            self.compress = reply.caps.iter().any(|cap| cap == session::CAP_LZ4);
            self.effects.engine_info = reply.engine_info;
        }
        if takeover {
            let route = socket_route(&self.sockets, id);
            if let Some(route) = route {
                self.table.record_switched(&route, now);
            }
        }
        self.become_current(id, now);
    }

    fn become_current(&mut self, id: SocketId, now: u64) {
        if let Some(old) = self.current.replace(id)
            && old != id
        {
            self.discard(old);
        }
        self.takeover = None;
        self.held.clear();
        self.old_socket_lost = false;
        let first = self.mode == Mode::Connecting;
        self.mode = Mode::Connected;
        if let Some(socket) = self.sockets.get_mut(&id) {
            socket.phase = Some(Phase::Live);
            let route = socket.route.clone();
            tuned(&route, &mut socket.heartbeat);
            let rtt = socket.heartbeat.last_rtt_ms();
            self.table.record_connected(&route, now);
            if let Some(rtt) = rtt {
                self.table.record_rtt(&route, rtt);
            }
        }
        self.effects.saved = Some(self.table.saved());
        self.effects.connected |= first;
        self.next_ack = now + session::ACK_INTERVAL_MS;
        self.next_probe = now + PROBE_TICK_MS;
    }

    fn application(&mut self, id: SocketId, payload: Vec<u8>, now: u64) {
        if self.session.is_some() {
            match session::parse_control(&payload) {
                Ok(Some(Control::Ack(count))) => {
                    if self.resume.acknowledge(count).is_err() {
                        self.lose(id, now);
                    }
                    return;
                }
                Ok(Some(Control::Routes(routes))) => {
                    self.learn(routes);
                    return;
                }
                Ok(Some(Control::End)) => {
                    self.end(LinkEnd::EndedByHost);
                    return;
                }
                Ok(None) => {}
                Err(_) => {
                    self.lose(id, now);
                    return;
                }
            }
        }
        let size = payload.len();
        let Ok(decoded) = session::decode_message(payload) else {
            self.lose(id, now);
            return;
        };
        self.effects.deliver.push(decoded);
        if self.session.is_some()
            && let Some(ack) = self.resume.record_received(size)
            && !self.seal_to(id, &ack)
        {
            self.lose(id, now);
        }
    }

    fn learn(&mut self, routes: Vec<Route>) {
        if self.table.learn(routes) {
            self.effects.saved = Some(self.table.saved());
        }
    }

    fn send_message(&mut self, message: &[u8], now: u64) -> bool {
        if matches!(self.mode, Mode::Ended | Mode::Connecting) {
            return false;
        }
        let wire = session::encode_message(message, self.compress);
        if self.session.is_none() {
            return self.current.is_some_and(|id| self.seal_to(id, &wire));
        }
        if self.resume.buffered() + wire.len() > MAX_BUFFERED_BYTES {
            self.end(LinkEnd::Lost);
            return false;
        }
        self.resume.record_sent(wire.clone());
        if self.takeover.is_some() {
            self.unwritten.push(wire);
        } else if self.mode == Mode::Connected
            && let Some(id) = self.current
            && !self.seal_to(id, &wire)
        {
            // Kept in the resume buffer; the replacement socket resends it.
            self.lose(id, now);
        }
        true
    }

    fn discard(&mut self, id: SocketId) {
        if let Some(mut socket) = self.sockets.remove(&id) {
            if let Some(channel) = &mut socket.channel {
                channel.close();
            }
            self.effects.close.push(id);
        }
    }

    /// The current socket failed: keep the session and find another route.
    fn lose(&mut self, id: SocketId, now: u64) {
        self.discard(id);
        if self.current == Some(id) {
            self.current = None;
        }
        if self.takeover.is_some() {
            self.old_socket_lost = true;
            self.held.clear();
            return;
        }
        if self.session.is_none() {
            self.end(LinkEnd::Lost);
            return;
        }
        for probe in self.socket_ids(Role::Probe) {
            self.discard(probe);
        }
        self.mode = Mode::Reattaching {
            deadline: now + session::CLIENT_RESUME_WINDOW_MS,
            next_attempt: now,
            backoff: REATTACH_BACKOFF_START_MS,
        };
        self.start_race(now);
    }

    fn socket_ids(&self, role: Role) -> Vec<SocketId> {
        self.sockets
            .iter()
            .filter(|(_, socket)| socket.role == role)
            .map(|(id, _)| *id)
            .collect()
    }

    fn drop_failed(&mut self, id: SocketId, authentication: bool, now: u64) {
        if self.current == Some(id) {
            self.lose(id, now);
            return;
        }
        let Some(socket) = self.sockets.get(&id) else {
            return;
        };
        let route = socket.route.clone();
        let role = socket.role;
        let was_takeover = self.takeover == Some(id);
        self.discard(id);
        if was_takeover {
            self.failed_takeover(&route, now);
            return;
        }
        self.table.record_failure(&route, now);
        if role == Role::Primary {
            self.authentication_failed |= authentication;
            if let Some(next) = self.queue.pop_front() {
                self.open_socket(next, Role::Primary, now);
                self.next_stagger = now + RACE_STAGGER_MS;
            } else if self.socket_ids(Role::Primary).is_empty() {
                self.race_failed(now);
            }
        }
    }

    fn failed_takeover(&mut self, route: &Route, now: u64) {
        self.table.record_failed_takeover(route, now);
        self.takeover = None;
        if self.old_socket_lost {
            self.old_socket_lost = false;
            if self.session.is_some() {
                self.mode = Mode::Reattaching {
                    deadline: now + session::CLIENT_RESUME_WINDOW_MS,
                    next_attempt: now,
                    backoff: REATTACH_BACKOFF_START_MS,
                };
                self.start_race(now);
            }
            return;
        }
        let Some(id) = self.current else { return };
        for record in std::mem::take(&mut self.held) {
            self.receive(id, record, now);
        }
        for wire in std::mem::take(&mut self.unwritten) {
            if !self.seal_to(id, &wire) {
                self.lose(id, now);
                return;
            }
        }
    }

    fn race_failed(&mut self, now: u64) {
        match self.mode {
            Mode::Connecting => self.end(if self.authentication_failed {
                LinkEnd::Authentication
            } else {
                LinkEnd::Unreachable
            }),
            Mode::Reattaching {
                deadline, backoff, ..
            } => {
                if now >= deadline {
                    self.end(LinkEnd::Lost);
                } else {
                    self.mode = Mode::Reattaching {
                        deadline,
                        next_attempt: now + backoff,
                        backoff: (backoff * 2).min(REATTACH_BACKOFF_MAX_MS),
                    };
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, reason: LinkEnd) {
        if self.mode == Mode::Ended {
            return;
        }
        self.mode = Mode::Ended;
        let ids: Vec<SocketId> = self.sockets.keys().copied().collect();
        for id in ids {
            self.discard(id);
        }
        self.queue.clear();
        self.current = None;
        self.takeover = None;
        self.effects.ended = Some(reason);
    }

    /// Everything time-driven, then the effects gathered during this call.
    fn finish(&mut self, now: u64) -> Effects {
        self.advance(now);
        self.effects.wait_ms = self.next_wait(now);
        let status = self.status();
        if self.published.as_ref() != Some(&status) {
            self.published = Some(status.clone());
            self.effects.status = Some(status);
        }
        std::mem::take(&mut self.effects)
    }

    fn advance(&mut self, now: u64) {
        if self.mode == Mode::Ended {
            return;
        }
        if !self.queue.is_empty() && now >= self.next_stagger {
            if let Some(next) = self.queue.pop_front() {
                self.open_socket(next, Role::Primary, now);
            }
            self.next_stagger = now + RACE_STAGGER_MS;
        }
        let expired: Vec<SocketId> = self
            .sockets
            .iter()
            .filter(|(id, socket)| Some(**id) != self.current && now >= socket.deadline)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.drop_failed(id, false, now);
        }
        if let Some(id) = self.current {
            self.drive_heartbeat(id, now);
        }
        if let Some(id) = self.current
            && self.session.is_some()
            && now >= self.next_ack
        {
            self.next_ack = now + session::ACK_INTERVAL_MS;
            if let Some(ack) = self.resume.pending_ack()
                && !self.seal_to(id, &ack)
            {
                self.lose(id, now);
            }
        }
        self.start_due_probes(now);
        if let Mode::Reattaching {
            deadline,
            next_attempt,
            backoff,
        } = self.mode
            && self.queue.is_empty()
            && self.socket_ids(Role::Primary).is_empty()
            && now >= next_attempt
        {
            if now >= deadline {
                self.end(LinkEnd::Lost);
            } else {
                self.mode = Mode::Reattaching {
                    deadline,
                    next_attempt: now + backoff,
                    backoff,
                };
                self.start_race(now);
            }
        }
    }

    fn drive_heartbeat(&mut self, id: SocketId, now: u64) {
        loop {
            let Some(socket) = self.sockets.get_mut(&id) else {
                return;
            };
            match socket.heartbeat.poll(now) {
                Ok(Action::Send(frame)) => {
                    if !self.seal_to(id, &frame) {
                        self.lose(id, now);
                        return;
                    }
                }
                Ok(Action::Wait(_)) => return,
                Ok(Action::Expired) | Err(_) => {
                    self.lose(id, now);
                    return;
                }
            }
        }
    }

    fn start_due_probes(&mut self, now: u64) {
        if self.mode != Mode::Connected
            || self.session.is_none()
            || self.takeover.is_some()
            || now < self.next_probe
            || !self.socket_ids(Role::Probe).is_empty()
        {
            return;
        }
        self.next_probe = now + PROBE_TICK_MS;
        let Some(current) = self.current.and_then(|id| socket_route(&self.sockets, id)) else {
            return;
        };
        for route in self.table.probes_due(&current, now, false) {
            self.open_socket(route, Role::Probe, now);
        }
    }

    fn next_wait(&mut self, now: u64) -> u64 {
        let mut at = now + IDLE_WAIT_MS;
        if self.mode == Mode::Ended {
            return IDLE_WAIT_MS;
        }
        if !self.queue.is_empty() {
            at = at.min(self.next_stagger);
        }
        for (id, socket) in &self.sockets {
            if Some(*id) != self.current {
                at = at.min(socket.deadline);
            }
        }
        if let Some(id) = self.current {
            if let Some(socket) = self.sockets.get_mut(&id)
                && let Ok(Action::Wait(ms)) = socket.heartbeat.poll(now)
            {
                at = at.min(now + ms);
            }
            if self.session.is_some() {
                at = at.min(self.next_ack);
            }
            let can_improve =
                socket_route(&self.sockets, id).is_some_and(|route| route.kind > RouteKind::Lan);
            if can_improve && self.mode == Mode::Connected {
                at = at.min(self.next_probe);
            }
        }
        if let Mode::Reattaching { next_attempt, .. } = self.mode {
            at = at.min(next_attempt);
        }
        at.saturating_sub(now).max(1)
    }

    fn status(&self) -> LinkStatus {
        let route = self.current.and_then(|id| socket_route(&self.sockets, id));
        LinkStatus {
            state: match self.mode {
                Mode::Connecting => LinkState::Connecting,
                Mode::Reattaching { .. } => LinkState::Reattaching,
                _ => LinkState::Connected,
            },
            rtt_ms: route.as_ref().and_then(|route| self.table.rtt(route)),
            route,
            resumable: self.session.is_some(),
            compressed: self.compress,
        }
    }
}

fn socket_route(sockets: &BTreeMap<SocketId, Socket>, id: SocketId) -> Option<Route> {
    sockets.get(&id).map(|socket| socket.route.clone())
}

/// `Ok(None)` is an older host's ordinary error reply.
fn parse_attach(payload: &[u8]) -> Result<Option<AttachReply>, ()> {
    let frame: serde_json::Value = serde_json::from_slice(payload).map_err(|_| ())?;
    if frame["id"].as_u64() != Some(session::ATTACH_ID) {
        return Err(());
    }
    if frame.get("err").is_some() {
        return Ok(None);
    }
    serde_json::from_value(frame["ok"].clone())
        .map(Some)
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accept, heartbeat};

    fn route(kind: RouteKind, origin: &str) -> Route {
        Route {
            kind,
            origin: origin.into(),
        }
    }
    fn relay() -> Route {
        route(RouteKind::Relay, "https://relay.example.com")
    }
    fn lan() -> Route {
        route(RouteKind::Lan, "http://192.168.1.5:27657")
    }
    fn saved_lan() -> SavedRoutes {
        SavedRoutes {
            learned: vec![lan()],
            ..Default::default()
        }
    }
    /// As [`saved_lan`], with the relay remembered as this network's winner.
    fn on_relay_with_lan_known() -> SavedRoutes {
        SavedRoutes {
            learned: vec![lan()],
            best_by_network: std::collections::HashMap::from([("home".to_owned(), relay().origin)]),
        }
    }

    /// The host's side of one socket, built from the real channel primitives.
    struct Host {
        invitation: Invitation,
        channel: Option<SecureChannel>,
        routes: Vec<Route>,
    }
    impl Host {
        fn new(invitation: &Invitation) -> Self {
            Self {
                invitation: invitation.clone(),
                channel: None,
                routes: vec![],
            }
        }
        fn handshake(&mut self, first: &[u8]) -> Vec<u8> {
            let (channel, reply) =
                accept(&self.invitation.id, &self.invitation.secret, first).unwrap();
            self.channel = Some(channel);
            reply
        }
        fn open(&mut self, record: &[u8]) -> Vec<u8> {
            self.channel
                .as_mut()
                .unwrap()
                .open(record)
                .unwrap()
                .unwrap()
        }
        fn seal(&mut self, payload: &[u8]) -> Vec<u8> {
            let mut records = self.channel.as_mut().unwrap().seal(payload).unwrap();
            assert_eq!(records.len(), 1);
            records.remove(0)
        }
        fn attach_ok(&mut self, resumed: bool, received: u64) -> Vec<u8> {
            let reply = AttachReply {
                session: "s".into(),
                resumed,
                received,
                caps: vec![],
                engine_info: Some(serde_json::json!({"deviceId":"host"})),
                routes: self.routes.clone(),
            };
            let frame = serde_json::json!({"id":session::ATTACH_ID,"ok":reply});
            self.seal(&serde_json::to_vec(&frame).unwrap())
        }
    }
    fn writes_to(effects: &Effects, id: SocketId) -> Vec<Vec<u8>> {
        effects
            .write
            .iter()
            .filter(|(socket, _)| *socket == id)
            .flat_map(|(_, records)| records.clone())
            .collect()
    }

    /// Take a link all the way to connected over its first socket.
    fn connected(saved: Option<SavedRoutes>) -> (Link, Host, Invitation) {
        let invitation = Invitation::generate().unwrap();
        let reported = saved
            .as_ref()
            .map(|s| s.learned.clone())
            .unwrap_or_default();
        let (mut link, effects) = Link::open(relay(), invitation.clone(), saved, "home", 0);
        let id = effects.open[0].0;
        let mut host = Host::new(&invitation);
        host.routes = reported;
        let opened = link.socket_opened(id, 1);
        let reply = host.handshake(&writes_to(&opened, id)[0]);
        let flight = link.record(id, reply, 2);
        let flight = writes_to(&flight, id);
        assert_eq!(flight.len(), 2, "heartbeat and attach share one flight");
        let answer = heartbeat::response(&host.open(&flight[0]))
            .unwrap()
            .unwrap();
        host.open(&flight[1]);
        let record = host.seal(&answer);
        link.record(id, record, 3);
        let record = host.attach_ok(false, 0);
        let effects = link.record(id, record, 4);
        assert!(effects.connected);
        assert_eq!(effects.engine_info.unwrap()["deviceId"], "host");
        (link, host, invitation)
    }

    #[test]
    fn later_routes_join_the_race_only_after_a_delay() {
        let invitation = Invitation::generate().unwrap();
        let (mut link, effects) = Link::open(relay(), invitation, Some(saved_lan()), "home", 0);
        assert_eq!(effects.open.len(), 1);
        assert!(effects.open[0].1.starts_with("ws://192.168.1.5"));
        assert!(effects.wait_ms <= RACE_STAGGER_MS);
        assert!(link.tick(50).open.is_empty());
        let later = link.tick(RACE_STAGGER_MS);
        assert_eq!(later.open.len(), 1);
        assert!(later.open[0].1.starts_with("wss://relay"));
    }

    #[test]
    fn the_first_route_through_noise_wins_and_the_rest_close() {
        let invitation = Invitation::generate().unwrap();
        let (mut link, first) =
            Link::open(relay(), invitation.clone(), Some(saved_lan()), "home", 0);
        let second = link.tick(RACE_STAGGER_MS);
        let (a, b) = (first.open[0].0, second.open[0].0);
        link.socket_opened(a, 201);
        let opened = link.socket_opened(b, 201);
        let mut host = Host::new(&invitation);
        let reply = host.handshake(&writes_to(&opened, b)[0]);
        let effects = link.record(b, reply, 202);
        assert_eq!(effects.close, vec![a]);
    }

    #[test]
    fn unreachable_and_bad_secrets_end_differently() {
        let invitation = Invitation::generate().unwrap();
        let (mut link, effects) = Link::open(relay(), invitation.clone(), None, "home", 0);
        let ended = link.socket_closed(effects.open[0].0, 1);
        assert_eq!(ended.ended, Some(LinkEnd::Unreachable));

        let (mut link, effects) = Link::open(relay(), invitation, None, "home", 0);
        let id = effects.open[0].0;
        let opened = link.socket_opened(id, 1);
        let other = Invitation::generate().unwrap();
        let reply = crate::initiate(&other.id, &other.secret).unwrap().1;
        let _ = opened;
        let ended = link.record(id, reply, 2);
        assert_eq!(ended.ended, Some(LinkEnd::Authentication));
    }

    #[test]
    fn an_older_host_works_without_a_session() {
        let invitation = Invitation::generate().unwrap();
        let (mut link, effects) = Link::open(relay(), invitation.clone(), None, "home", 0);
        let id = effects.open[0].0;
        let mut host = Host::new(&invitation);
        let opened = link.socket_opened(id, 1);
        let reply = host.handshake(&writes_to(&opened, id)[0]);
        let flight = writes_to(&link.record(id, reply, 2), id);
        let answer = heartbeat::response(&host.open(&flight[0]))
            .unwrap()
            .unwrap();
        host.open(&flight[1]);
        let record = host.seal(&answer);
        link.record(id, record, 3);
        let frame = serde_json::json!({"id":session::ATTACH_ID,"err":"unknown method"});
        let record = host.seal(&serde_json::to_vec(&frame).unwrap());
        let effects = link.record(id, record, 4);
        assert!(effects.connected);
        assert!(!effects.status.unwrap().resumable);
        let ended = link.socket_closed(id, 5);
        assert_eq!(ended.ended, Some(LinkEnd::Lost));
    }

    #[test]
    fn a_lost_socket_resumes_and_resends_only_what_is_missing() {
        let (mut link, _, invitation) = connected(None);
        let (accepted, _) = link.send(b"{\"id\":1}", 10);
        assert!(accepted);
        let (accepted, _) = link.send(b"{\"id\":2}", 11);
        assert!(accepted);
        let current = link.current.unwrap();
        let effects = link.socket_closed(current, 12);
        assert_eq!(effects.status.unwrap().state, LinkState::Reattaching);
        // Sent while no socket exists: queued for the replacement.
        assert!(link.send(b"{\"id\":3}", 13).0);

        let id = effects.open[0].0;
        let mut host = Host::new(&invitation);
        let opened = link.socket_opened(id, 14);
        let reply = host.handshake(&writes_to(&opened, id)[0]);
        let flight = writes_to(&link.record(id, reply, 15), id);
        let answer = heartbeat::response(&host.open(&flight[0]))
            .unwrap()
            .unwrap();
        let request: serde_json::Value = serde_json::from_slice(&host.open(&flight[1])).unwrap();
        assert_eq!(request["params"]["session"], "s");
        let record = host.seal(&answer);
        link.record(id, record, 16);
        // The host had already received the first message.
        let record = host.attach_ok(true, 1);
        let effects = link.record(id, record, 17);
        assert_eq!(effects.status.clone().unwrap().state, LinkState::Connected);
        let resent: Vec<Vec<u8>> = writes_to(&effects, id)
            .iter()
            .map(|r| host.open(r))
            .collect();
        assert_eq!(resent, vec![b"{\"id\":2}".to_vec(), b"{\"id\":3}".to_vec()]);
    }

    #[test]
    fn a_session_the_host_forgot_ends_the_link() {
        let (mut link, _, invitation) = connected(None);
        let effects = link.socket_closed(link.current.unwrap(), 10);
        let id = effects.open[0].0;
        let mut host = Host::new(&invitation);
        let opened = link.socket_opened(id, 11);
        let reply = host.handshake(&writes_to(&opened, id)[0]);
        let flight = writes_to(&link.record(id, reply, 12), id);
        let answer = heartbeat::response(&host.open(&flight[0]))
            .unwrap()
            .unwrap();
        host.open(&flight[1]);
        let record = host.seal(&answer);
        link.record(id, record, 13);
        let record = host.attach_ok(false, 0);
        assert_eq!(link.record(id, record, 14).ended, Some(LinkEnd::Lost));
    }

    #[test]
    fn reattaching_stops_at_the_resume_window() {
        let (mut link, _, _) = connected(None);
        let mut effects = link.socket_closed(link.current.unwrap(), 10);
        let mut now = 10;
        for _ in 0..2_000 {
            for (id, _) in std::mem::take(&mut effects.open) {
                effects = link.socket_closed(id, now);
            }
            if effects.ended.is_some() {
                break;
            }
            now += 100;
            effects = link.tick(now);
            if effects.ended.is_some() {
                break;
            }
        }
        assert_eq!(effects.ended, Some(LinkEnd::Lost));
        assert!(now >= session::CLIENT_RESUME_WINDOW_MS);
    }

    #[test]
    fn the_host_ending_a_session_is_final() {
        let (mut link, mut host, _) = connected(None);
        let id = link.current.unwrap();
        let record = host.seal(&session::end_frame());
        assert_eq!(
            link.record(id, record, 10).ended,
            Some(LinkEnd::EndedByHost)
        );
    }

    #[test]
    fn delivers_compressed_messages_and_acknowledges() {
        let (mut link, mut host, _) = connected(None);
        let id = link.current.unwrap();
        let text = br#"{"id":1,"item":"streamed"}"#.repeat(100);
        let record = host.seal(&session::encode_message(&text, true));
        let effects = link.record(id, record, 10);
        assert_eq!(effects.deliver, vec![text]);
        let effects = link.tick(2_000);
        let acks: Vec<_> = writes_to(&effects, id)
            .iter()
            .map(|r| host.open(r))
            .collect();
        assert!(acks.contains(&session::ack_frame(1)));
    }

    #[test]
    fn learned_routes_are_saved_for_the_next_launch() {
        let (mut link, mut host, _) = connected(None);
        let id = link.current.unwrap();
        let record = host.seal(&session::routes_frame(&[lan()]).unwrap());
        let effects = link.record(id, record, 10);
        assert_eq!(effects.saved.unwrap().learned, vec![lan()]);
    }

    /// Drive one probe socket through Noise and a heartbeat with this latency.
    fn answer_probe(
        link: &mut Link,
        id: SocketId,
        invitation: &Invitation,
        at: u64,
        rtt: u64,
    ) -> Effects {
        let mut host = Host::new(invitation);
        let opened = link.socket_opened(id, at);
        let reply = host.handshake(&writes_to(&opened, id)[0]);
        let challenge = writes_to(&link.record(id, reply, at), id);
        let answer = heartbeat::response(&host.open(&challenge[0]))
            .unwrap()
            .unwrap();
        let record = host.seal(&answer);
        link.record(id, record, at + rtt)
    }

    #[test]
    fn a_clearly_faster_route_takes_over_after_two_good_probes() {
        let (mut link, mut old_host, invitation) = connected(Some(on_relay_with_lan_known()));
        let old = link.current.unwrap();
        link.table.record_rtt(&relay(), 200);
        let probe = link.tick(10_000);
        let id = probe.open[0].0;
        answer_probe(&mut link, id, &invitation, 10_000, 5);
        let probe = link.tick(13_100);
        let id = probe.open[0].0;

        let mut host = Host::new(&invitation);
        host.routes = vec![lan()];
        let opened = link.socket_opened(id, 13_100);
        let reply = host.handshake(&writes_to(&opened, id)[0]);
        let challenge = writes_to(&link.record(id, reply, 13_100), id);
        let answer = heartbeat::response(&host.open(&challenge[0]))
            .unwrap()
            .unwrap();
        let record = host.seal(&answer);
        let flight = link.record(id, record, 13_105);
        let request: serde_json::Value =
            serde_json::from_slice(&host.open(&writes_to(&flight, id)[0])).unwrap();
        assert_eq!(request["params"]["session"], "s");

        // Traffic during the move is neither lost nor read twice.
        assert!(link.send(b"{\"id\":9}", 13_106).0);
        let late = old_host.seal(br#"{"late":true}"#);
        assert!(link.record(old, late, 13_107).deliver.is_empty());

        let record = host.attach_ok(true, 0);
        let effects = link.record(id, record, 13_110);
        assert_eq!(effects.close, vec![old]);
        assert_eq!(
            effects.status.clone().unwrap().route.unwrap().kind,
            RouteKind::Lan
        );
        let resent: Vec<_> = writes_to(&effects, id)
            .iter()
            .map(|r| host.open(r))
            .collect();
        assert_eq!(resent, vec![b"{\"id\":9}".to_vec()]);
    }

    #[test]
    fn a_failed_takeover_keeps_the_old_socket_and_its_traffic() {
        let (mut link, mut old_host, invitation) = connected(Some(on_relay_with_lan_known()));
        let old = link.current.unwrap();
        link.table.record_rtt(&relay(), 200);
        let id = link.tick(10_000).open[0].0;
        // Confirming also checks the old route, which its host must read.
        let confirming = answer_probe(&mut link, id, &invitation, 10_000, 5);
        for record in writes_to(&confirming, old) {
            old_host.open(&record);
        }
        let probe = link.tick(13_100);
        for record in writes_to(&probe, old) {
            old_host.open(&record);
        }
        let id = probe.open[0].0;
        answer_probe(&mut link, id, &invitation, 13_100, 5);
        assert_eq!(link.takeover, Some(id));

        assert!(link.send(b"{\"id\":9}", 13_106).0);
        let late = old_host.seal(br#"{"late":true}"#);
        link.record(old, late, 13_107);

        let effects = link.socket_closed(id, 13_110);
        assert_eq!(effects.deliver, vec![br#"{"late":true}"#.to_vec()]);
        assert_eq!(old_host.open(&writes_to(&effects, old)[0]), b"{\"id\":9}");
        assert_eq!(link.current, Some(old));
        assert!(effects.ended.is_none());
    }

    #[test]
    fn a_network_change_checks_the_socket_and_looks_for_better_routes() {
        let (mut link, _, _) = connected(Some(on_relay_with_lan_known()));
        link.table.record_rtt(&relay(), 200);
        link.table.record_failure(&lan(), 0);
        assert!(link.tick(100).open.is_empty());
        let effects = link.network_changed("elsewhere", 200);
        assert!(!effects.write.is_empty(), "an immediate heartbeat is due");
        assert_eq!(effects.open.len(), 1);
    }

    #[test]
    fn an_unanswered_heartbeat_replaces_the_socket() {
        let (mut link, _, _) = connected(None);
        link.tick(21_000);
        let effects = link.tick(36_001);
        assert_eq!(effects.status.unwrap().state, LinkState::Reattaching);
    }

    #[test]
    fn sends_are_refused_before_connecting_and_after_ending() {
        let invitation = Invitation::generate().unwrap();
        let (mut link, _) = Link::open(relay(), invitation, None, "home", 0);
        assert!(!link.send(b"{}", 1).0);
        let ended = link.close(2);
        assert_eq!(ended.ended, Some(LinkEnd::Closed));
        assert!(!link.send(b"{}", 3).0);
    }
}
