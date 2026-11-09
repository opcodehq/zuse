//! Bounded browser socket ownership shared by workspace RPC and paired uploads.
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, closure::Closure, prelude::wasm_bindgen};
use web_sys::{MessageEvent, WebSocket};
use zuse_remote::{
    Invitation, MAX_MESSAGE_BYTES, MAX_RECORD_BYTES, PairedTarget, Route, RouteTable,
    SecureChannel,
    heartbeat::{Action, Heartbeat},
    routes::Decision,
    session::{self, AttachReply, AttachRequest, Control, Resume},
};

#[wasm_bindgen(module = "/src/rpc_compression.js")]
extern "C" {
    #[wasm_bindgen(js_name = supportsRpcCompression)]
    fn supports_rpc_compression() -> bool;
    #[wasm_bindgen(js_name = createRpcDecoder)]
    fn create_rpc_decoder() -> wasm_bindgen::JsValue;
    #[wasm_bindgen(catch, js_name = decodeRpcFrame)]
    async fn decode_rpc_frame(
        bytes: js_sys::Uint8Array,
        max_bytes: u32,
        decoder: wasm_bindgen::JsValue,
    ) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}

// WebSocket.send rejects views backed by shared WASM memory. Own a normal JS
// buffer for the browser call; do not pass a borrowed Rust slice directly.
fn binary(socket: &WebSocket, bytes: &[u8]) -> Result<(), wasm_bindgen::JsValue> {
    let owned = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
    owned.copy_from(bytes);
    socket.send_with_array_buffer(&owned.buffer())
}

enum Incoming {
    Text(String),
    Binary(Vec<u8>),
    Closed,
}
pub(super) struct Socket {
    socket: WebSocket,
    incoming: mpsc::Receiver<Incoming>,
    closed: Rc<Cell<bool>>,
    channel: Option<SecureChannel>,
    heartbeat: Option<Heartbeat>,
    account: Option<Account>,
    session: Option<Session>,
    clock: web_time::Instant,
    plain_probe: zuse_protocol::rpc_probe::Probe,
    // Connection.send can cancel receive. Keep an in-progress decode owned
    // by the socket so no already-dequeued frame disappears on that branch.
    decoder: wasm_bindgen::JsValue,
    pending_text: Option<futures::future::LocalBoxFuture<'static, Result<String, String>>>,
    _open: Closure<dyn FnMut()>,
    _close: Closure<dyn FnMut()>,
    _error: Closure<dyn FnMut()>,
    _message: Closure<dyn FnMut(MessageEvent)>,
}
impl Socket {
    pub fn account_dependency_blocked(&self) -> bool {
        self.account
            .as_ref()
            .is_some_and(|account| account.controller.is_blocked())
    }
    pub async fn verify_environment(&mut self, environment: &str) -> Result<(), String> {
        let verify = async {
            self.send(&serde_json::json!({"id":0,"method":zuse_protocol::methods::ENGINE_INFO,"params":{}}).to_string())?;
            let text = self.receive().await?;
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|_| "Invalid computer identity")?;
            if value["id"] != 0 || value["ok"]["deviceId"].as_str() != Some(environment) {
                return Err("The selected computer returned another identity".to_owned());
            }
            Ok(())
        };
        futures::select_biased! {value=verify.fuse()=>value,_=super::connection::delay(10000).fuse()=>Err("Computer identity verification timed out".into())}
    }

    pub fn account_origin(&self) -> Option<String> {
        self.account.as_ref().map(|a| {
            a.origin
                .trim_end_matches("/api/account-rpc")
                .replacen("wss://", "https://", 1)
                .replacen("ws://", "http://", 1)
        })
    }
    pub async fn connect(url: &str, invitation: Option<&Invitation>) -> Result<Self, String> {
        Self::connect_with_protocol(url, invitation, None).await
    }
    async fn connect_with_protocol(
        url: &str,
        invitation: Option<&Invitation>,
        protocol: Option<&str>,
    ) -> Result<Self, String> {
        // Opt-in query negotiation lets older engines continue replying with
        // plain text. Secure and account channels retain their own framing.
        let compressed_url = (protocol.is_none()
            && invitation.is_none()
            && supports_rpc_compression()
            && !url.contains("rpcCompression="))
        .then(|| {
            format!(
                "{url}{}rpcCompression=deflate-stream-v1",
                if url.contains('?') { '&' } else { '?' }
            )
        });
        let url = compressed_url.as_deref().unwrap_or(url);
        let socket = match protocol {
            Some(protocol) => WebSocket::new_with_str(url, protocol),
            None => WebSocket::new(url),
        }
        .map_err(|_| "Could not open engine connection")?;
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let (ready_tx, ready_rx) = oneshot::channel();
        let ready = Rc::new(RefCell::new(Some(ready_tx)));
        let signal = ready.clone();
        let open = Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = signal.borrow_mut().take() {
                let _ = tx.send(true);
            }
        });
        let closed = Rc::new(Cell::new(false));
        let (sender, incoming) = mpsc::channel(32);
        let mut sender_close = sender.clone();
        let signal = ready.clone();
        let state = closed.clone();
        let close = Closure::<dyn FnMut()>::new(move || {
            state.set(true);
            if let Some(tx) = signal.borrow_mut().take() {
                let _ = tx.send(false);
            }
            let _ = sender_close.try_send(Incoming::Closed);
        });
        let mut sender_error = sender.clone();
        let state = closed.clone();
        let error_socket = socket.clone();
        let error = Closure::<dyn FnMut()>::new(move || {
            state.set(true);
            if let Some(tx) = ready.borrow_mut().take() {
                let _ = tx.send(false);
            }
            let _ = sender_error.try_send(Incoming::Closed);
            let _ = error_socket.close();
        });
        let state = closed.clone();
        let message_socket = socket.clone();
        let mut sender = sender;
        let binary_limit = if invitation.is_none() && protocol.is_none() {
            // A valid incompressible stream has a small framing overhead.
            // Decoding still enforces the original uncompressed byte cap.
            MAX_MESSAGE_BYTES + 64 * 1024
        } else {
            MAX_RECORD_BYTES
        };
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let data = event.data();
            let frame = if let Some(text) = data.as_string() {
                (text.len() <= MAX_MESSAGE_BYTES).then_some(Incoming::Text(text))
            } else if data.is_instance_of::<js_sys::ArrayBuffer>() {
                let bytes = js_sys::Uint8Array::new(&data);
                (bytes.length() as usize <= binary_limit).then(|| Incoming::Binary(bytes.to_vec()))
            } else {
                None
            };
            if frame.is_none_or(|frame| sender.try_send(frame).is_err()) {
                state.set(true);
                let _ = message_socket.close();
                let _ = sender.try_send(Incoming::Closed);
            }
        });
        socket.set_onopen(Some(open.as_ref().unchecked_ref()));
        socket.set_onclose(Some(close.as_ref().unchecked_ref()));
        socket.set_onerror(Some(error.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
        let mut owner = Self {
            socket,
            incoming,
            closed,
            channel: None,
            heartbeat: None,
            account: None,
            session: None,
            clock: web_time::Instant::now(),
            plain_probe: Default::default(),
            decoder: create_rpc_decoder(),
            pending_text: None,
            _open: open,
            _close: close,
            _error: error,
            _message: message,
        };
        let ready = futures::select_biased! {
            ready = ready_rx.fuse() => ready == Ok(true),
            _ = super::connection::delay(5_000).fuse() => false,
        };
        if !ready {
            return Err("Could not reach engine".into());
        }
        if let Some(invitation) = invitation {
            let (handshake, first) = zuse_remote::initiate(&invitation.id, &invitation.secret)
                .map_err(|_| "Could not start paired authentication")?;
            binary(&owner.socket, &first).map_err(|_| "Paired connection closed")?;
            let reply = futures::select_biased! {
                reply = owner.raw().fuse() => reply?,
                _ = super::connection::delay(10_000).fuse() => return Err("Paired authentication timed out".into()),
            };
            let Incoming::Binary(reply) = reply else {
                return Err("Paired authentication failed".into());
            };
            owner.channel = Some(
                handshake
                    .finish(&reply)
                    .map_err(|_| "Paired authentication failed")?,
            );
            owner.clock = web_time::Instant::now();
            owner.heartbeat = Some(Heartbeat::new());
            owner.poll_heartbeat()?;
            let confirmation = async {
                loop {
                    let Incoming::Binary(record) = owner.raw().await? else {
                        return Err(
                            "Paired host did not confirm responsiveness. Update the host.".into(),
                        );
                    };
                    let payload = owner
                        .channel
                        .as_mut()
                        .unwrap()
                        .open(&record)
                        .map_err(|_| "Paired host authentication failed")?;
                    if let Some(payload) = payload {
                        let now = owner.clock.elapsed().as_millis() as u64;
                        if owner
                            .heartbeat
                            .as_mut()
                            .unwrap()
                            .consume(&payload, now)
                            .map_err(
                                |_| "Paired host did not confirm responsiveness. Update the host.",
                            )?
                        {
                            return Ok::<_, String>(());
                        }
                        return Err("Paired host sent an invalid confirmation".into());
                    }
                }
            };
            futures::select_biased! {
                result = confirmation.fuse() => result?,
                _ = super::connection::delay(zuse_remote::heartbeat::INITIAL_TIMEOUT_MS as u32).fuse() => return Err("Paired host did not confirm responsiveness. Update the host.".into()),
            }
        }
        Ok(owner)
    }
    fn poll_heartbeat(&mut self) -> Result<Option<u64>, String> {
        if self.heartbeat.is_none() && self.account.is_none() {
            return match self
                .plain_probe
                .poll(self.clock.elapsed().as_millis() as u64)
            {
                zuse_protocol::rpc_probe::Action::Send => {
                    self.send(zuse_protocol::rpc_probe::REQUEST)?;
                    self.poll_heartbeat()
                }
                zuse_protocol::rpc_probe::Action::Wait(ms) => Ok(Some(ms)),
                zuse_protocol::rpc_probe::Action::Expired => {
                    Err("Engine stopped responding".into())
                }
            };
        }
        loop {
            let Some(heartbeat) = &mut self.heartbeat else {
                return Ok(None);
            };
            match heartbeat
                .poll(self.clock.elapsed().as_millis() as u64)
                .map_err(|error| error.to_string())?
            {
                Action::Send(payload) => self.send_secure(&payload)?,
                Action::Wait(ms) => return Ok(Some(ms)),
                Action::Expired => return Err("Paired engine stopped responding".into()),
            }
        }
    }
    fn send_secure(&mut self, payload: &[u8]) -> Result<(), String> {
        if self.closed.get() || self.socket.ready_state() != WebSocket::OPEN {
            return Err("Paired connection closed".into());
        }
        if self.socket.buffered_amount() as usize + payload.len() > 20 * 1024 * 1024 {
            self.closed.set(true);
            let _ = self.socket.close();
            return Err("Paired connection is too slow".into());
        }
        for record in self
            .channel
            .as_mut()
            .ok_or("Paired channel unavailable")?
            .seal(payload)
            .map_err(|_| "Paired message could not be encrypted")?
        {
            binary(&self.socket, &record).map_err(|_| "Paired connection closed")?;
        }
        Ok(())
    }
    async fn raw(&mut self) -> Result<Incoming, String> {
        if self.closed.get() {
            return Err("Engine connection closed".into());
        }
        match self.incoming.next().await {
            Some(Incoming::Closed) | None => Err("Engine connection closed".into()),
            Some(frame) => Ok(frame),
        }
    }
    pub fn send(&mut self, text: &str) -> Result<(), String> {
        if self
            .account
            .as_ref()
            .is_some_and(|a| a.controller.is_closed() || a.until <= js_sys::Date::now() as i64)
        {
            return Err("Account connection authorization ended".into());
        }
        if self.closed.get() || self.socket.ready_state() != WebSocket::OPEN {
            return Err("Engine connection closed".into());
        }
        // Synchronous JS sends must not accumulate an unlimited native queue.
        if self.socket.buffered_amount() as usize + text.len() > 20 * 1024 * 1024 {
            self.closed.set(true);
            let _ = self.socket.close();
            return Err("Engine connection is too slow".into());
        }
        if self.channel.is_some() {
            if let Some(session) = &mut self.session {
                let wire = session::encode_message(text.as_bytes(), session.compress);
                session.resume.record_sent(wire.clone());
                // A failed write is kept and resent after reattaching.
                let _ = self.send_secure(&wire);
                return Ok(());
            }
            self.send_secure(text.as_bytes())?;
        } else {
            self.socket
                .send_with_str(text)
                .map_err(|_| "Engine connection closed")?;
        }
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<String, String> {
        if self.account.is_some() {
            return self.receive_account().await;
        }
        loop {
            self.send_due_ack()?;
            let frame = if let Some(pending) = self.pending_text.as_mut() {
                let decoded = pending.await;
                self.pending_text = None;
                Incoming::Text(decoded?)
            } else if let Some(ms) = self.poll_heartbeat()?.map(|ms| {
                if self.session.is_some() {
                    ms.min(session::ACK_INTERVAL_MS)
                } else {
                    ms
                }
            }) {
                futures::select_biased! {
                    frame = self.raw().fuse() => frame?,
                    _ = super::connection::delay(ms as u32).fuse() => continue,
                }
            } else {
                self.raw().await?
            };
            match (&mut self.channel, frame) {
                (None, Incoming::Binary(bytes)) => {
                    let owned = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
                    owned.copy_from(&bytes);
                    let decoder = self.decoder.clone();
                    self.pending_text = Some(
                        async move {
                            decode_rpc_frame(owned, MAX_MESSAGE_BYTES as u32, decoder)
                                .await
                                .map_err(|_| "Invalid compressed engine frame".to_string())?
                                .as_string()
                                .ok_or_else(|| "Invalid compressed engine text".to_string())
                        }
                        .boxed_local(),
                    );
                }
                (None, Incoming::Text(text)) => {
                    if self.plain_probe.awaiting_reply() {
                        for line in text.lines() {
                            if let Ok(frame) = serde_json::from_str::<serde_json::Value>(line)
                                && frame["id"] == 0
                                && frame["ok"]["deviceId"]
                                    .as_str()
                                    .is_some_and(|id| !id.is_empty())
                            {
                                self.plain_probe
                                    .acknowledge(self.clock.elapsed().as_millis() as u64);
                            }
                        }
                        // The dispatcher ignores id=0. Preserve all envelopes
                        // when a probe shares a compressed batch with updates.
                    }
                    return Ok(text);
                }
                (Some(channel), Incoming::Binary(bytes)) => {
                    if let Some(message) = channel
                        .open(&bytes)
                        .map_err(|_| "Paired authentication failed")?
                    {
                        if let Some(heartbeat) = &mut self.heartbeat {
                            if heartbeat
                                .consume(&message, self.clock.elapsed().as_millis() as u64)
                                .map_err(|error| error.to_string())?
                            {
                                self.record_rtt();
                                continue;
                            }
                        }
                        if self.session.is_none() {
                            return String::from_utf8(message)
                                .map_err(|_| "Invalid engine message".into());
                        }
                        match self.receive_session(message)? {
                            Some(text) => return Ok(text),
                            None => continue,
                        }
                    }
                }
                _ => return Err("Engine sent an unexpected transport frame".into()),
            }
        }
    }
}
/// A paired session that outlives this WebSocket; see `zuse_remote::session`.
struct Session {
    id: String,
    resume: Resume,
    compress: bool,
    route: Route,
    table: Rc<RefCell<RouteTable>>,
    invitation: Invitation,
    ack_at: web_time::Instant,
    ended: bool,
}
const RACE_STAGGER_MS: u32 = 200;
pub(super) type ProbeResult = (Route, Option<(Socket, u32)>);

fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}
impl Socket {
    /// Browsers can only open HTTPS routes from an HTTPS page, so the race
    /// and later probes are limited to those.
    async fn race(
        table: &Rc<RefCell<RouteTable>>,
        invitation: &Invitation,
    ) -> Result<(Socket, Route), String> {
        use futures::stream::FuturesUnordered;
        let mut queue = std::collections::VecDeque::from(table.borrow().dial_order(true));
        let mut pending = FuturesUnordered::new();
        let start = |route: Route, pending: &mut FuturesUnordered<_>| {
            let invitation = invitation.clone();
            pending.push(
                async move {
                    let url = PairedTarget::for_route(&route.origin, invitation.clone())
                        .map_err(|_| "Invalid route".to_owned())?
                        .socket_url();
                    let socket = Self::connect(&url, Some(&invitation)).await?;
                    Ok::<_, String>((socket, route))
                }
                .boxed_local(),
            );
        };
        let first = queue.pop_front().ok_or("No route to the paired engine")?;
        start(first, &mut pending);
        let mut last_error = "Could not reach engine".to_owned();
        loop {
            futures::select_biased! {
                finished = pending.next().fuse() => match finished {
                    Some(Ok(winner)) => return Ok(winner),
                    Some(Err(error)) => {
                        last_error = error;
                        if let Some(next) = queue.pop_front() {
                            start(next, &mut pending);
                        } else if pending.is_empty() {
                            return Err(last_error);
                        }
                    }
                    None => return Err(last_error),
                },
                _ = super::connection::delay(RACE_STAGGER_MS).fuse() => {
                    if let Some(next) = queue.pop_front() {
                        start(next, &mut pending);
                    }
                },
            }
        }
    }

    /// Open a session over the fastest route that answers.
    pub async fn connect_paired(
        table: Rc<RefCell<RouteTable>>,
        invitation: &Invitation,
    ) -> Result<Self, String> {
        let (mut socket, route) = Self::race(&table, invitation).await?;
        table.borrow_mut().record_connected(&route, now_ms());
        // A host that predates sessions answers with an error and stays usable.
        if let Some(reply) = socket
            .exchange_attach(&AttachRequest {
                session: None,
                received: 0,
                caps: vec![session::CAP_LZ4.into()],
            })
            .await?
        {
            table.borrow_mut().learn(reply.routes.clone());
            socket.session = Some(Session {
                id: reply.session.clone(),
                resume: Resume::default(),
                compress: reply.caps.iter().any(|cap| cap == session::CAP_LZ4),
                route,
                table,
                invitation: invitation.clone(),
                ack_at: web_time::Instant::now(),
                ended: false,
            });
        }
        Ok(socket)
    }

    async fn exchange_attach(
        &mut self,
        request: &AttachRequest,
    ) -> Result<Option<AttachReply>, String> {
        let frame = serde_json::json!({
            "id": session::ATTACH_ID,
            "method": session::ATTACH_METHOD,
            "params": request,
        })
        .to_string();
        self.send_secure(frame.as_bytes())?;
        let reply = async {
            loop {
                let Incoming::Binary(record) = self.raw().await? else {
                    return Err("Unexpected transport frame".to_owned());
                };
                let channel = self.channel.as_mut().ok_or("Paired channel unavailable")?;
                let Some(payload) = channel
                    .open(&record)
                    .map_err(|_| "Paired authentication failed")?
                else {
                    continue;
                };
                let value: serde_json::Value =
                    serde_json::from_slice(&payload).map_err(|_| "Invalid attach reply")?;
                if value["id"].as_u64() != Some(session::ATTACH_ID) {
                    return Err("Invalid attach reply".to_owned());
                }
                if value.get("err").is_some() {
                    return Ok(None);
                }
                return serde_json::from_value(value["ok"].clone())
                    .map(Some)
                    .map_err(|_| "Invalid attach reply".to_owned());
            }
        };
        futures::select_biased! {
            reply = reply.fuse() => reply,
            _ = super::connection::delay(10_000).fuse() => Err("Paired host did not attach".into()),
        }
    }

    /// True when a lost socket can be replaced without disturbing the session.
    pub fn resumable(&self) -> bool {
        self.session.as_ref().is_some_and(|s| !s.ended)
    }

    /// The address uploads should use while this route is active.
    pub fn current_target(&self) -> Option<PairedTarget> {
        let session = self.session.as_ref()?;
        PairedTarget::for_route(&session.route.origin, session.invitation.clone()).ok()
    }

    /// What the interface shows about the route in use.
    pub fn route_status(&self) -> Option<serde_json::Value> {
        let session = self.session.as_ref()?;
        Some(serde_json::json!({
            "kind": session.route.kind,
            "label": session.route.kind.label(),
            "rttMs": session.table.borrow().rtt(&session.route),
        }))
    }

    fn record_rtt(&mut self) {
        let rtt = self.heartbeat.as_ref().and_then(Heartbeat::last_rtt_ms);
        if let (Some(rtt), Some(session)) = (rtt, &self.session) {
            session.table.borrow_mut().record_rtt(&session.route, rtt);
        }
    }

    fn send_due_ack(&mut self) -> Result<(), String> {
        let Some(session) = &mut self.session else {
            return Ok(());
        };
        if session.ack_at.elapsed().as_millis() < session::ACK_INTERVAL_MS as u128 {
            return Ok(());
        }
        session.ack_at = web_time::Instant::now();
        match session.resume.pending_ack() {
            Some(ack) => self.send_secure(&ack),
            None => Ok(()),
        }
    }

    fn receive_session(&mut self, message: Vec<u8>) -> Result<Option<String>, String> {
        let session = self.session.as_mut().ok_or("Paired session unavailable")?;
        match session::parse_control(&message).map_err(|_| "Invalid session frame")? {
            Some(Control::Ack(count)) => {
                session
                    .resume
                    .acknowledge(count)
                    .map_err(|_| "Invalid session frame")?;
                return Ok(None);
            }
            Some(Control::Routes(routes)) => {
                session.table.borrow_mut().learn(routes);
                return Ok(None);
            }
            Some(Control::End) => {
                session.ended = true;
                return Err("Paired session ended".into());
            }
            None => {}
        }
        let size = message.len();
        let decoded = session::decode_message(message).map_err(|_| "Invalid engine message")?;
        let text = String::from_utf8(decoded).map_err(|_| "Invalid engine message")?;
        if let Some(ack) = session.resume.record_received(size) {
            self.send_secure(&ack)?;
        }
        Ok(Some(text))
    }

    /// Move this session onto `next`. False means the host no longer has it.
    async fn adopt(&mut self, mut next: Socket, route: Route) -> Result<bool, String> {
        let mut carried = self.session.take().ok_or("Paired session unavailable")?;
        let reply = next
            .exchange_attach(&AttachRequest {
                session: Some(carried.id.clone()),
                received: carried.resume.received(),
                caps: vec![session::CAP_LZ4.into()],
            })
            .await;
        let reply = match reply {
            Ok(Some(reply)) => reply,
            Ok(None) | Err(_) => {
                self.session = Some(carried);
                return Err("Paired host did not attach".into());
            }
        };
        if !reply.resumed {
            return Ok(false);
        }
        carried.table.borrow_mut().learn(reply.routes.clone());
        let replay = carried
            .resume
            .rewind(reply.received)
            .map_err(|_| "Invalid session state")?;
        for wire in replay {
            next.send_secure(&wire)?;
        }
        carried.route = route;
        next.session = Some(carried);
        *self = next;
        Ok(true)
    }

    /// Replace a lost socket within the host's holding window.
    pub async fn reattach(&mut self) -> bool {
        let deadline = now_ms() + session::CLIENT_RESUME_WINDOW_MS;
        let mut backoff = 100;
        let Some((table, invitation)) = self
            .session
            .as_ref()
            .map(|s| (s.table.clone(), s.invitation.clone()))
        else {
            return false;
        };
        while now_ms() < deadline {
            if let Ok((next, route)) = Self::race(&table, &invitation).await {
                match self.adopt(next, route.clone()).await {
                    Ok(true) => {
                        table.borrow_mut().record_connected(&route, now_ms());
                        return true;
                    }
                    Ok(false) => return false,
                    Err(_) => {}
                }
            }
            super::connection::delay(backoff).await;
            backoff = (backoff * 2).min(2_000);
        }
        false
    }

    /// A probe to run beside the session when one is due. It owns everything
    /// it needs, so the session keeps working while it is in flight.
    pub fn due_probe(&self) -> Option<futures::future::LocalBoxFuture<'static, ProbeResult>> {
        let session = self.session.as_ref()?;
        let route = session
            .table
            .borrow()
            .probes_due(&session.route, now_ms(), true)
            .into_iter()
            .next()?;
        let invitation = session.invitation.clone();
        Some(
            async move {
                let measured = async {
                    let url = PairedTarget::for_route(&route.origin, invitation.clone())
                        .ok()?
                        .socket_url();
                    let socket = Self::connect(&url, Some(&invitation)).await.ok()?;
                    let rtt = socket.heartbeat.as_ref()?.last_rtt_ms()?;
                    Some((socket, rtt))
                }
                .await;
                (route, measured)
            }
            .boxed_local(),
        )
    }

    /// Apply a probe result, moving the session if the route clearly won.
    pub async fn finish_probe(&mut self, result: ProbeResult) -> bool {
        let (route, measured) = result;
        let Some(session) = &self.session else {
            return true;
        };
        let table = session.table.clone();
        let current = session.route.clone();
        let Some((candidate, rtt)) = measured else {
            table.borrow_mut().record_failure(&route, now_ms());
            return true;
        };
        let decision = table
            .borrow_mut()
            .judge_probe(&current, &route, rtt, now_ms());
        if decision != Decision::Switch {
            return true;
        }
        match self.adopt(candidate, route.clone()).await {
            Ok(alive) => {
                if alive {
                    table.borrow_mut().record_switched(&route, now_ms());
                }
                alive
            }
            Err(_) => {
                table.borrow_mut().record_failed_takeover(&route, now_ms());
                true
            }
        }
    }
}
struct Account {
    controller: crate::account_connection::Controller,
    origin: String,
    until: i64,
    renew_at: f64,
    renewal: Option<
        futures::future::LocalBoxFuture<
            'static,
            Result<zuse_protocol::managed_connection::Bootstrap, String>,
        >,
    >,
    awaiting: Option<(zuse_protocol::managed_connection::Bootstrap, f64)>,
    failures: u32,
    ping_at: f64,
    pong: Option<(String, f64)>,
}
fn account_url(grant: &zuse_protocol::managed_connection::Bootstrap) -> Result<String, String> {
    let origin =
        zuse_remote::normalized_origin(&grant.origin).map_err(|_| "Invalid computer endpoint")?;
    if origin != grant.origin.trim_end_matches('/')
        || grant.connect_token.is_empty()
        || grant.connect_token.len() > 16384
        || grant.expires_at <= js_sys::Date::now() as i64
        || grant.expires_at > js_sys::Date::now() as i64 + 86400000
    {
        return Err("Invalid account connection authorization".into());
    }
    let base = if let Some(host) = origin.strip_prefix("https://") {
        format!("wss://{host}")
    } else {
        format!(
            "ws://{}",
            origin
                .strip_prefix("http://")
                .ok_or("Invalid computer endpoint")?
        )
    };
    Ok(format!("{base}/api/account-rpc"))
}
fn ack(
    text: &str,
    grant: &zuse_protocol::managed_connection::Bootstrap,
    renewed: bool,
) -> Result<i64, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "Invalid account acknowledgement")?;
    let until = v["expiresAt"].as_i64().ok_or("Invalid account expiry")?;
    if v["authenticated"] != true
        || v["protocol"] != zuse_protocol::managed_connection::PROTOCOL
        || v["deviceId"] != grant.environment_id
        || v["renewed"].as_bool().unwrap_or(false) != renewed
        || until <= js_sys::Date::now() as i64
        || until > grant.expires_at.saturating_add(1000)
    {
        return Err("Account authorization was not confirmed".into());
    }
    Ok(until)
}
impl Socket {
    pub async fn connect_account(
        controller: crate::account_connection::Controller,
    ) -> Result<Self, String> {
        let grant = controller.grant().await?;
        let url = account_url(&grant)?;
        // Explicit subprotocol opts only this grant-authenticated endpoint into
        // cross-origin browser transport. No cookie authorizes an account socket.
        let mut owner = Self::connect_with_protocol(
            &url,
            None,
            Some(zuse_protocol::managed_connection::PROTOCOL),
        )
        .await?;
        owner.send(&serde_json::json!({"protocol":zuse_protocol::managed_connection::PROTOCOL,"connectToken":grant.connect_token}).to_string())?;
        let first = futures::select_biased! {value=owner.raw().fuse()=>value?,_=super::connection::delay(10000).fuse()=>return Err("Account authentication timed out".into())};
        let Incoming::Text(first) = first else {
            return Err("Invalid account authentication response".into());
        };
        let until = ack(&first, &grant, false)?;
        let now = js_sys::Date::now();
        owner.account = Some(Account {
            controller,
            origin: url,
            until,
            renew_at: now
                + zuse_protocol::managed_connection::renew_in_ms(until, now as i64).unwrap_or(1)
                    as f64,
            renewal: None,
            awaiting: None,
            failures: 0,
            ping_at: now + 20000.,
            pong: None,
        });
        Ok(owner)
    }
    async fn receive_account(&mut self) -> Result<String, String> {
        loop {
            let now = js_sys::Date::now();
            let account = self.account.as_mut().unwrap();
            if account.controller.is_closed() || account.until <= now as i64 {
                return Err("Account connection authorization ended".into());
            }
            if account.awaiting.as_ref().is_some_and(|(_, at)| *at <= now)
                || account.pong.as_ref().is_some_and(|(_, at)| *at <= now)
            {
                return Err("Account computer stopped responding".into());
            }
            if account.renewal.is_none() && account.awaiting.is_none() && now >= account.renew_at {
                let controller = account.controller.clone();
                account.renewal = Some(async move { controller.grant().await }.boxed_local());
            }
            if account.pong.is_none() && now >= account.ping_at {
                let nonce = uuid::Uuid::new_v4().to_string();
                account.pong = Some((nonce.clone(), now + 10000.));
                self.send(&serde_json::json!({"protocol":zuse_protocol::managed_connection::PROTOCOL,"ping":nonce}).to_string())?;
            }
            enum Next {
                Frame(Result<Incoming, String>),
                Grant(Result<zuse_protocol::managed_connection::Bootstrap, String>),
                Tick,
            }
            let account = self.account.as_mut().unwrap();
            let incoming = &mut self.incoming;
            let closed = self.closed.clone();
            let event = futures::select! {
                frame=incoming.next().fuse()=>Next::Frame(match frame {Some(Incoming::Closed)|None=>Err("Account computer connection closed".into()),Some(frame) if !closed.get()=>Ok(frame),_=>Err("Account computer connection closed".into())}),
                grant=async {match &mut account.renewal {Some(future)=>future.await,None=>futures::future::pending().await}}.fuse()=>Next::Grant(grant),
                _=super::connection::delay(1000).fuse()=>Next::Tick,
            };
            let account = self.account.as_mut().unwrap();
            match event {
                Next::Tick => continue,
                Next::Grant(result) => {
                    account.renewal = None;
                    match result {
                        Ok(grant) => {
                            if account_url(&grant)? != account.origin {
                                return Err(
                                    "The computer endpoint changed. Reopen it from Remote access."
                                        .into(),
                                );
                            }
                            if grant.expires_at > account.until.saturating_add(1000) {
                                let frame=serde_json::json!({"protocol":zuse_protocol::managed_connection::PROTOCOL,"connectToken":grant.connect_token}).to_string();
                                account.awaiting = Some((grant, js_sys::Date::now() + 10000.));
                                self.send(&frame)?;
                                continue;
                            }
                        }
                        Err(_) if account.controller.is_closed() => {
                            return Err("The controller account connection ended".into());
                        }
                        Err(_) => {}
                    }
                    account.failures = account.failures.saturating_add(1);
                    account.renew_at = js_sys::Date::now()
                        + ((1u64 << account.failures.min(5)).min(30) * 1000) as f64;
                }
                Next::Frame(result) => {
                    let Incoming::Text(text) = result? else {
                        return Err("Unexpected account transport frame".into());
                    };
                    // Control headers are small. Canonical frames are forwarded untouched.
                    if let Ok(value) =
                        serde_json::from_str::<zuse_protocol::managed_connection::Control>(&text)
                    {
                        if let Some(protocol) = value.protocol {
                            if protocol != zuse_protocol::managed_connection::PROTOCOL {
                                return Err("Invalid account control protocol".into());
                            }
                            if let Some(nonce) = value.pong {
                                if account
                                    .pong
                                    .as_ref()
                                    .is_none_or(|(expected, _)| expected != nonce)
                                {
                                    return Err("Invalid computer heartbeat".into());
                                }
                                account.pong = None;
                                account.ping_at = js_sys::Date::now() + 20000.;
                                continue;
                            }
                            let (grant, _) = account
                                .awaiting
                                .take()
                                .ok_or("Unexpected account acknowledgement")?;
                            let until = ack(&text, &grant, true)?;
                            if until <= account.until {
                                return Err("Account authorization did not renew".into());
                            }
                            account.until = until;
                            account.failures = 0;
                            account.renew_at = js_sys::Date::now()
                                + zuse_protocol::managed_connection::renew_in_ms(
                                    until,
                                    js_sys::Date::now() as i64,
                                )
                                .ok_or("Account authorization expired")?
                                    as f64;
                            continue;
                        }
                    }
                    return Ok(text);
                }
            }
        }
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        if let Ok(close) = js_sys::Reflect::get(&self.decoder, &"close".into()) {
            if let Some(close) = close.dyn_ref::<js_sys::Function>() {
                let _ = close.call0(&self.decoder);
            }
        }
        self.socket.set_onopen(None);
        self.socket.set_onclose(None);
        self.socket.set_onerror(None);
        self.socket.set_onmessage(None);
        let _ = self.socket.close();
        if let Some(channel) = &mut self.channel {
            channel.close();
        }
    }
}
