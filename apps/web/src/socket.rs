//! Bounded browser socket ownership shared by workspace RPC and paired uploads.
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{MessageEvent, WebSocket};
use zuse_remote::{
    Invitation, MAX_MESSAGE_BYTES, MAX_RECORD_BYTES, SecureChannel,
    heartbeat::{Action, Heartbeat},
};

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
    clock: web_time::Instant,
    _open: Closure<dyn FnMut()>,
    _close: Closure<dyn FnMut()>,
    _error: Closure<dyn FnMut()>,
    _message: Closure<dyn FnMut(MessageEvent)>,
}
impl Socket {
    pub async fn connect(url: &str, invitation: Option<&Invitation>) -> Result<Self, String> {
        let socket = WebSocket::new(url).map_err(|_| "Could not open engine connection")?;
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
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let data = event.data();
            let frame = if let Some(text) = data.as_string() {
                (text.len() <= MAX_MESSAGE_BYTES).then_some(Incoming::Text(text))
            } else if data.is_instance_of::<js_sys::ArrayBuffer>() {
                let bytes = js_sys::Uint8Array::new(&data);
                (bytes.length() as usize <= MAX_RECORD_BYTES)
                    .then(|| Incoming::Binary(bytes.to_vec()))
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
            clock: web_time::Instant::now(),
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
            self.send_secure(text.as_bytes())?;
        } else {
            self.socket
                .send_with_str(text)
                .map_err(|_| "Engine connection closed")?;
        }
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<String, String> {
        loop {
            let wait = self.poll_heartbeat()?;
            let frame = if let Some(ms) = wait {
                futures::select_biased! {
                    frame = self.raw().fuse() => frame?,
                    _ = super::connection::delay(ms as u32).fuse() => continue,
                }
            } else {
                self.raw().await?
            };
            match (&mut self.channel, frame) {
                (None, Incoming::Text(text)) => return Ok(text),
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
                                continue;
                            }
                        }
                        return String::from_utf8(message)
                            .map_err(|_| "Invalid engine message".into());
                    }
                }
                _ => return Err("Engine sent an unexpected transport frame".into()),
            }
        }
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
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
