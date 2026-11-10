//! The paired link for the browser client.
//!
//! [`zuse_remote::link::Link`] decides everything: racing routes, measuring
//! them, moving the session and resuming it after a loss. The page owns the
//! WebSockets and the timer and reports what happens to them; each call hands
//! back the effects to carry out. Pairing secrets never leave this module
//! except through [`export_link`], which the page keeps in its own storage.
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use zuse_remote::{
    Invitation, PairedTarget, Route,
    link::{Effects, Link, LinkEnd},
    routes::{SavedRoutes, classify_origin},
};

fn set(target: &Object, key: &str, value: impl Into<JsValue>) {
    let _ = Reflect::set(target, &JsValue::from_str(key), &value.into());
}

fn now(value: f64) -> u64 {
    if value.is_finite() && value > 0.0 {
        value as u64
    } else {
        0
    }
}

fn end_name(end: LinkEnd) -> &'static str {
    match end {
        LinkEnd::Unreachable => "unreachable",
        LinkEnd::Authentication => "authentication",
        LinkEnd::Lost => "lost",
        LinkEnd::EndedByHost => "endedByHost",
        LinkEnd::Closed => "closed",
    }
}

fn effects(effects: Effects) -> JsValue {
    let out = Object::new();
    let open = Array::new();
    for (socket, url) in effects.open {
        let entry = Object::new();
        set(&entry, "socket", socket as f64);
        set(&entry, "url", url);
        open.push(&entry);
    }
    set(&out, "open", open);
    let close = Array::new();
    for socket in effects.close {
        close.push(&JsValue::from_f64(socket as f64));
    }
    set(&out, "close", close);
    let write = Array::new();
    for (socket, records) in effects.write {
        let entry = Object::new();
        set(&entry, "socket", socket as f64);
        let list = Array::new();
        for record in records {
            // A fresh JS buffer: WebSocket.send rejects views of WASM memory.
            list.push(&Uint8Array::from(record.as_slice()));
        }
        set(&entry, "records", list);
        write.push(&entry);
    }
    set(&out, "write", write);
    let deliver = Array::new();
    for message in effects.deliver {
        // RPC frames are JSON text; anything else is not an engine message.
        if let Ok(text) = String::from_utf8(message) {
            deliver.push(&JsValue::from_str(&text));
        }
    }
    set(&out, "deliver", deliver);
    set(&out, "waitMs", effects.wait_ms as f64);
    if let Some(status) = effects.status {
        let entry = Object::new();
        set(&entry, "state", status.state.as_str());
        if let Some(route) = &status.route {
            set(
                &entry,
                "kind",
                serde_json::to_value(route.kind)
                    .ok()
                    .and_then(|kind| kind.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            );
            set(&entry, "label", route.kind.label());
        }
        if let Some(rtt) = status.rtt_ms {
            set(&entry, "rttMs", rtt as f64);
        }
        set(&entry, "resumable", status.resumable);
        set(&entry, "compressed", status.compressed);
        set(&out, "status", entry);
    }
    if let Some(saved) = effects.saved
        && let Ok(text) = serde_json::to_string(&saved)
    {
        set(&out, "saved", text);
    }
    if let Some(info) = effects.engine_info {
        set(&out, "engineInfo", info.to_string());
    }
    set(&out, "connected", effects.connected);
    if let Some(end) = effects.ended {
        set(&out, "ended", end_name(end));
    }
    out.into()
}

#[wasm_bindgen]
pub struct WebLink {
    link: Link,
    initial: Option<Effects>,
}

#[wasm_bindgen]
impl WebLink {
    /// Start connecting to the computer in an exported pairing link. `saved`
    /// is the route memory from an earlier session, as JSON.
    pub fn open(
        link: &str,
        saved: Option<String>,
        network: &str,
        now_ms: f64,
    ) -> Result<WebLink, JsError> {
        let target =
            PairedTarget::parse_link(link).map_err(|_| JsError::new("Invalid pairing link"))?;
        let invited = Route {
            kind: classify_origin(target.origin()),
            origin: target.origin().to_owned(),
        };
        let saved = saved.and_then(|text| serde_json::from_str::<SavedRoutes>(&text).ok());
        let (link, initial) = Link::open(
            invited,
            target.invitation().clone(),
            saved,
            network,
            now(now_ms),
        );
        Ok(WebLink {
            link,
            initial: Some(initial),
        })
    }

    /// The effects produced by [`WebLink::open`]. Empty after the first call.
    pub fn start(&mut self) -> JsValue {
        effects(self.initial.take().unwrap_or_default())
    }

    #[wasm_bindgen(js_name = socketOpened)]
    pub fn socket_opened(&mut self, socket: f64, now_ms: f64) -> JsValue {
        effects(self.link.socket_opened(socket as u64, now(now_ms)))
    }

    pub fn record(&mut self, socket: f64, record: &[u8], now_ms: f64) -> JsValue {
        if record.len() > zuse_remote::MAX_RECORD_BYTES {
            return effects(self.link.socket_closed(socket as u64, now(now_ms)));
        }
        effects(
            self.link
                .record(socket as u64, record.to_vec(), now(now_ms)),
        )
    }

    #[wasm_bindgen(js_name = socketClosed)]
    pub fn socket_closed(&mut self, socket: f64, now_ms: f64) -> JsValue {
        effects(self.link.socket_closed(socket as u64, now(now_ms)))
    }

    /// Queue one RPC frame. `accepted` is false when the link cannot take it.
    pub fn send(&mut self, message: &str, now_ms: f64) -> JsValue {
        if message.len() > zuse_remote::MAX_MESSAGE_BYTES {
            let out = effects(self.link.tick(now(now_ms)));
            let _ = Reflect::set(&out, &"accepted".into(), &false.into());
            return out;
        }
        let (accepted, produced) = self.link.send(message.as_bytes(), now(now_ms));
        let out = effects(produced);
        let _ = Reflect::set(&out, &"accepted".into(), &accepted.into());
        out
    }

    pub fn tick(&mut self, now_ms: f64) -> JsValue {
        effects(self.link.tick(now(now_ms)))
    }

    #[wasm_bindgen(js_name = networkChanged)]
    pub fn network_changed(&mut self, network: &str, now_ms: f64) -> JsValue {
        effects(self.link.network_changed(network, now(now_ms)))
    }

    pub fn close(&mut self, now_ms: f64) -> JsValue {
        effects(self.link.close(now(now_ms)))
    }
}

/// The public parts of an exported pairing link: `{ origin, id }`.
#[wasm_bindgen(js_name = inspectLink)]
pub fn inspect_link(link: &str) -> Result<JsValue, JsError> {
    let target =
        PairedTarget::parse_link(link).map_err(|_| JsError::new("Invalid pairing link"))?;
    let out = Object::new();
    set(&out, "origin", target.origin());
    set(&out, "id", target.invitation().id.as_str());
    set(
        &out,
        "kind",
        serde_json::to_value(classify_origin(target.origin()))
            .ok()
            .and_then(|kind| kind.as_str().map(str::to_owned))
            .unwrap_or_default(),
    );
    Ok(out.into())
}

/// Turn a `#pair=` fragment opened on `origin` into an exported link.
#[wasm_bindgen(js_name = linkFromFragment)]
pub fn link_from_fragment(origin: &str, fragment: &str) -> Result<String, JsError> {
    let invitation = Invitation::parse_fragment(fragment)
        .map_err(|_| JsError::new("Invalid pairing invitation"))?;
    let target = PairedTarget::new(origin, invitation)
        .map_err(|_| JsError::new("Pairing requires HTTPS, or localhost for development"))?;
    export_link(&target)
}

/// Normalize a pasted link so only well-formed capabilities are stored.
#[wasm_bindgen(js_name = normalizeLink)]
pub fn normalize_link(link: &str) -> Result<String, JsError> {
    let target = PairedTarget::parse_link(link.trim()).map_err(|_| {
        JsError::new("Enter a valid private connection link from the other computer.")
    })?;
    export_link(&target)
}

fn export_link(target: &PairedTarget) -> Result<String, JsError> {
    target
        .export_link()
        .map_err(|_| JsError::new("Invalid pairing invitation"))
}
