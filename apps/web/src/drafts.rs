//! Per-engine, per-tab journals. Other tabs' records are read as copies only.
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use zuse_protocol::browser_drafts::{DraftJournal, MAX_DRAFTS, MAX_JOURNAL_BYTES};

thread_local! { static TAB: RefCell<Option<(String, Option<String>)>> = const { RefCell::new(None) }; }

async fn claim(owner: &str) -> Option<bool> {
    let window = web_sys::window()?;
    let manager = js_sys::Reflect::get(window.navigator().as_ref(), &"locks".into()).ok()?;
    if manager.is_null() || manager.is_undefined() {
        return None;
    }
    let request = js_sys::Reflect::get(&manager, &"request".into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    let options = js_sys::Object::new();
    js_sys::Reflect::set(&options, &"ifAvailable".into(), &JsValue::TRUE).ok()?;
    let (send, receive) = futures::channel::oneshot::channel();
    let callback = Closure::once(move |lock: JsValue| -> JsValue {
        let acquired = !lock.is_null() && !lock.is_undefined();
        let _ = send.send(acquired);
        if acquired {
            // Hold this browser-owned lock until the document closes. A duplicate
            // tab inherits sessionStorage but cannot acquire the same journal.
            js_sys::Promise::new(&mut |_, _| {}).into()
        } else {
            js_sys::Promise::resolve(&JsValue::NULL).into()
        }
    });
    let promise = request
        .call3(
            &manager,
            &format!("zuse.draft-owner.{owner}").into(),
            &options,
            callback.as_ref(),
        )
        .ok()?
        .dyn_into::<js_sys::Promise>()
        .ok()?;
    callback.forget();
    use futures::FutureExt;
    futures::select_biased! {
        result = receive.fuse() => result.ok(),
        _ = JsFuture::from(promise).fuse() => None,
    }
}
pub(super) async fn initialize() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(storage) = window.session_storage().ok().flatten() else {
        return;
    };
    let previous = storage
        .get_item(OWNER)
        .ok()
        .flatten()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok());
    let candidate = previous
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let owner = match claim(&candidate).await {
        Some(true) => candidate,
        Some(false) => {
            let owner = uuid::Uuid::new_v4().to_string();
            if claim(&owner).await != Some(true) {
                return;
            }
            owner
        }
        // Without Web Locks, always allocate a fresh record. The previous record
        // remains a read-only backup rather than risking another tab's journal.
        None => uuid::Uuid::new_v4().to_string(),
    };
    if storage.set_item(OWNER, &owner).is_err() {
        return;
    }
    let source = previous.filter(|previous| previous != &owner);
    TAB.with(|tab| *tab.borrow_mut() = Some((owner, source)));
}

const PREFIX: &str = "zuse.browser-drafts.v1.";
const OWNER: &str = "zuse.browser-drafts.owner.v1";
fn stores(device: &str) -> Result<(web_sys::Storage, String, String), &'static str> {
    uuid::Uuid::parse_str(device).map_err(|_| "Engine identity is invalid")?;
    let window = web_sys::window().ok_or("Browser window is unavailable")?;
    let (owner, _) = TAB
        .with(|tab| tab.borrow().clone())
        .ok_or("Tab draft identity is unavailable")?;
    let storage = window
        .local_storage()
        .ok()
        .flatten()
        .ok_or("Local storage is unavailable")?;
    let prefix = format!("{PREFIX}{device}.");
    let key = format!("{prefix}{owner}");
    Ok((storage, prefix, key))
}
pub(super) fn handle(frame: &Value, events: &UnboundedSender<Value>) -> bool {
    let action = frame["ui"].as_str().unwrap_or_default();
    if !matches!(action, "loadDrafts" | "saveDrafts") {
        return false;
    }
    let device = frame["deviceId"].as_str().unwrap_or_default();
    let result = (|| -> Result<Value, &'static str> {
        let (storage, prefix, key) = stores(device)?;
        if action == "saveDrafts" {
            let journal = DraftJournal::decode(&frame["value"].to_string())?;
            let text = journal.encode()?;
            storage
                .set_item(&key, &text)
                .map_err(|_| "Drafts could not be saved; storage may be full or disabled")?;
            return Ok(json!({"deviceId":device,"saved":true}));
        }
        let source = TAB.with(|tab| tab.borrow().as_ref().and_then(|(_, source)| source.clone()));
        let read_key = source
            .map(|source| format!("{prefix}{source}"))
            .unwrap_or_else(|| key.clone());
        let stored = storage
            .get_item(&read_key)
            .map_err(|_| "Drafts could not be read")?;
        let journal = match stored.as_deref() {
            Some(text) => DraftJournal::decode(text)?,
            None => DraftJournal::default(),
        };
        let mut copies = DraftJournal::default();
        let mut warning = None;
        // Existing tabs restore their own journal. A fresh tab can recover copies
        // from closed sessions without clearing or taking ownership of a peer's data.
        if stored.is_none() {
            let count = storage
                .length()
                .map_err(|_| "Draft records could not be listed")?;
            if count > 4096 {
                warning =
                    Some("Some older browser draft records remain outside the recovery scan limit");
            }
            let mut bytes = 0usize;
            'records: for index in 0..count.min(4096) {
                let Some(other) = storage
                    .key(index)
                    .map_err(|_| "Draft records could not be listed")?
                else {
                    continue;
                };
                if other == key || !other.starts_with(&prefix) {
                    continue;
                }
                let Some(text) = storage
                    .get_item(&other)
                    .map_err(|_| "Draft copies could not be read")?
                else {
                    continue;
                };
                bytes = bytes.saturating_add(text.len());
                if bytes > MAX_JOURNAL_BYTES || copies.recovery.len() >= MAX_DRAFTS {
                    warning = Some(
                        "Some older browser drafts remain in storage; too many copies to open at once",
                    );
                    break;
                }
                match DraftJournal::decode(&text) {
                    Ok(other) => {
                        for draft in other.drafts.into_iter().chain(other.recovery) {
                            if copies.recovery.len() >= MAX_DRAFTS {
                                warning = Some(
                                    "Some older browser drafts remain in storage; too many copies to open at once",
                                );
                                break 'records;
                            }
                            copies.keep_recovery(draft);
                        }
                    }
                    Err(_) => {
                        warning = Some(
                            "An older browser draft record could not be read; it has been retained in storage",
                        )
                    }
                }
            }
        }
        Ok(json!({"deviceId":device,"journal":journal,"copies":copies.recovery,"error":warning}))
    })();
    let reply = result.unwrap_or_else(|error| json!({"deviceId":device,"error":error}));
    let _ = events.unbounded_send(json!({"browserDrafts":reply}));
    true
}
