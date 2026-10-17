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
use zuse_remote::{Invitation, MAX_MESSAGE_BYTES, MAX_RECORD_BYTES, SecureChannel};

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
        }
        Ok(owner)
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
        if let Some(channel) = &mut self.channel {
            for record in channel
                .seal(text.as_bytes())
                .map_err(|_| "Paired message could not be encrypted")?
            {
                binary(&self.socket, &record).map_err(|_| "Paired connection closed")?;
            }
        } else {
            self.socket
                .send_with_str(text)
                .map_err(|_| "Engine connection closed")?;
        }
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<String, String> {
        loop {
            let frame = self.raw().await?;
            match (&mut self.channel, frame) {
                (None, Incoming::Text(text)) => return Ok(text),
                (Some(channel), Incoming::Binary(bytes)) => {
                    if let Some(message) = channel
                        .open(&bytes)
                        .map_err(|_| "Paired authentication failed")?
                    {
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
