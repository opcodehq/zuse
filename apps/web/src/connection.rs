//! Browser socket lifetime and request admission. Only the view reopens reads.
use futures::{FutureExt, StreamExt, channel::mpsc};
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};
use zuse_protocol::{
    browser_link::{BrowserLink, retry_delay_ms},
    methods,
};
use zuse_remote::{Invitation, PairedTarget};

type Sender = mpsc::UnboundedSender<Value>;
pub(super) async fn delay(ms: u32) {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        if let Err(error) = web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32)
        {
            let _ = reject.call1(&wasm_bindgen::JsValue::NULL, &error);
        }
    });
    let _ = JsFuture::from(promise).await;
}
async fn auth_status() -> Result<bool, ()> {
    let controller = web_sys::AbortController::new().map_err(|_| ())?;
    let abort = controller.clone();
    let timeout = Closure::<dyn FnMut()>::new(move || abort.abort());
    let window = web_sys::window().unwrap();
    let timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            timeout.as_ref().unchecked_ref(),
            3_000,
        )
        .map_err(|_| ())?;
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    init.set_signal(Some(&controller.signal()));
    let request = Request::new_with_str_and_init("/api/session/check", &init).map_err(|_| ())?;
    let response = JsFuture::from(window.fetch_with_request(&request)).await;
    window.clear_timeout_with_handle(timer);
    let response: Response = response.map_err(|_| ())?.dyn_into().map_err(|_| ())?;
    match response.status() {
        200..=299 | 404 | 405 => Ok(true),
        401 | 403 => Ok(false),
        _ => Err(()),
    }
}
fn local_action(frame: &Value, events: &Sender) -> bool {
    if crate::drafts::handle(frame, events) {
        return true;
    }
    if frame["ui"] == "playCompletionSound" {
        if let Ok(preset) = serde_json::from_value(frame["preset"].clone()) {
            crate::audio::play(preset);
        }
        return true;
    }
    if frame["ui"] == "forgetPairing" {
        if let Ok(Some(storage)) = web_sys::window().unwrap().session_storage() {
            let _ = storage.remove_item("zuse.paired-target.v1");
        }
        let _ = web_sys::window().unwrap().location().reload();
        return true;
    }
    if frame["ui"] != "savePreferences" {
        return false;
    }
    if let Some(value) = frame.get("value") {
        let text = value.to_string();
        if text.len() <= 64 * 1024 {
            if let Ok(Some(storage)) = web_sys::window().unwrap().local_storage() {
                let _ = storage.set_item("zuse.workspace.preferences.v1", &text);
            }
        }
    }
    true
}
fn reject(events: &Sender, frame: &Value) {
    crate::attachments::discard(frame);
    if let Some(id) = frame["id"].as_u64() {
        if frame["cancel"] != true {
            let _=events.unbounded_send(json!({"id":id,"err":"The engine is not connected. This request was not sent.","outcomeUnknown":false}));
        }
    } else if frame["ui"] == "pickAttachments" || frame["ui"] == "pasteAttachments" {
        let _=events.unbounded_send(json!({"attachment":{"error":"Reconnect to the engine before uploading files."},"generation":frame["generation"],"epoch":frame["epoch"],"uploadRejected":true}));
    }
}
async fn wait_offline(
    ms: u32,
    outbound: &mut mpsc::UnboundedReceiver<Value>,
    events: &Sender,
) -> bool {
    let timer = delay(ms).fuse();
    futures::pin_mut!(timer);
    loop {
        futures::select_biased! {
            _=timer=>return true,
            frame=outbound.next().fuse()=>match frame {Some(frame)=>{if !local_action(&frame, events){reject(events,&frame)}},None=>return false},
        }
    }
}
fn mutation(frame: &Value) -> bool {
    !methods::retryable_read(
        frame["method"].as_str().unwrap_or_default(),
        &frame["params"],
    )
}

fn paired_target() -> Result<Option<PairedTarget>, String> {
    let window = web_sys::window().unwrap();
    let location = window.location();
    let hash = location
        .hash()
        .map_err(|_| "Could not read pairing invitation")?;
    let storage = window.session_storage().ok().flatten();
    if hash.starts_with("#token=") {
        if let Some(storage) = &storage {
            let _ = storage.remove_item("zuse.paired-target.v1");
        }
        return Ok(None);
    }
    if hash.starts_with("#pair=") {
        // Remove the capability even when its contents are invalid.
        let clean = format!(
            "{}{}",
            location.pathname().unwrap_or_else(|_| "/".into()),
            location.search().unwrap_or_default()
        );
        window
            .history()
            .map_err(|_| "Could not clear private invitation")?
            .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&clean))
            .map_err(|_| "Could not clear private invitation")?;
        if let Some(storage) = &storage {
            let _ = storage.remove_item("zuse.paired-target.v1");
        }
        let invitation =
            Invitation::parse_fragment(&hash).map_err(|_| "Invalid pairing invitation")?;
        let target = PairedTarget::new(
            &location.origin().map_err(|_| "Invalid engine origin")?,
            invitation,
        )
        .map_err(|_| "Pairing requires HTTPS, or localhost for development")?;
        if let Some(storage) = storage {
            let _ = storage.set_item(
                "zuse.paired-target.v1",
                &target
                    .export_link()
                    .map_err(|_| "Invalid pairing invitation")?,
            );
        }
        return Ok(Some(target));
    }
    let saved =
        storage.and_then(|storage| storage.get_item("zuse.paired-target.v1").ok().flatten());
    saved
        .map(|link| {
            PairedTarget::parse_link(&link).map_err(|_| {
                "Saved pairing is invalid. Forget it and open a new invitation.".into()
            })
        })
        .transpose()
}

pub(super) async fn run(mut outbound: mpsc::UnboundedReceiver<Value>, events: Sender) {
    crate::drafts::initialize().await;
    let target = match paired_target() {
        Ok(target) => target,
        Err(message) => {
            let _ = events.unbounded_send(
                json!({"connection":"pairingRequired","message":message,"paired":true}),
            );
            while wait_offline(8_000, &mut outbound, &events).await {}
            return;
        }
    };
    let paired = target.is_some();
    if !paired {
        if let Err(error) = super::browser::unlock().await {
            let _=events.unbounded_send(json!({"connection":"authRequired","message":error.as_string().unwrap_or_else(||"Authentication required".into())}));
        }
    }
    let location = web_sys::window().unwrap().location();
    let local = !paired
        && matches!(
            location.hostname().unwrap_or_default().as_str(),
            "localhost" | "127.0.0.1" | "[::1]" | "::1"
        );
    let _ = events.unbounded_send(json!({"browserLocal":local,"paired":paired,"pairingOrigin":target.as_ref().map(|target|target.origin().to_owned()).unwrap_or_else(||location.origin().unwrap_or_default())}));
    let scheme = if location.protocol().unwrap_or_default() == "https:" {
        "wss"
    } else {
        "ws"
    };
    let url = target
        .as_ref()
        .map(PairedTarget::socket_url)
        .unwrap_or_else(|| format!("{scheme}://{}/api/rpc", location.host().unwrap_or_default()));
    let mut link = BrowserLink::default();
    let mut attempt = 0;
    loop {
        if !paired {
            match auth_status().await {
                Ok(false) => {
                    let _ = events.unbounded_send(json!({"connection":"authRequired"}));
                    if !wait_offline(8_000, &mut outbound, &events).await {
                        return;
                    }
                    continue;
                }
                Err(()) => {
                    let _ = events.unbounded_send(json!({"connection":"retrying"}));
                    if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
                        return;
                    }
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                Ok(true) => {}
            }
        }
        let socket =
            super::socket::Socket::connect(&url, target.as_ref().map(PairedTarget::invitation))
                .await;
        let mut socket = match socket {
            Ok(socket) => socket,
            Err(message) => {
                let _ = events.unbounded_send(
                    json!({"connection":"retrying","paired":paired,"message":message}),
                );
                if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
                    return;
                }
                attempt = attempt.saturating_add(1);
                continue;
            }
        };
        let epoch = link.open();
        attempt = 0;
        let _ =
            events.unbounded_send(json!({"connection":"connected","epoch":epoch,"paired":paired}));
        let mut application_closed = false;
        loop {
            enum Next {
                Inbound(Result<String, String>),
                Outbound(Option<Value>),
            }
            let next = futures::select_biased! {
                text = socket.receive().fuse() => Next::Inbound(text),
                frame = outbound.next().fuse() => Next::Outbound(frame),
            };
            let frame = match next {
                Next::Inbound(Ok(text)) => {
                    for line in text.lines() {
                        if let Ok(frame) = serde_json::from_str::<Value>(line) {
                            if let Some(id) = frame["id"].as_u64() {
                                let terminal = frame.get("ok").is_some()
                                    || frame.get("err").is_some()
                                    || frame["done"] == true;
                                if link.receive(epoch, id, terminal) {
                                    let _ = events.unbounded_send(frame);
                                }
                            }
                        }
                    }
                    continue;
                }
                Next::Inbound(Err(_)) => break,
                Next::Outbound(Some(frame)) => frame,
                Next::Outbound(None) => {
                    application_closed = true;
                    break;
                }
            };
            if local_action(&frame, &events) {
                continue;
            }
            if frame["ui"] == "pickAttachments" || frame["ui"] == "pasteAttachments" {
                if frame["epoch"].as_u64() == Some(epoch) {
                    if frame["ui"] == "pasteAttachments" {
                        crate::attachments::pasted(&frame, events.clone(), target.clone());
                        continue;
                    }
                    crate::attachments::pick_attachments(
                        events.clone(),
                        frame["generation"].as_u64().unwrap_or_default(),
                        epoch,
                        target.clone(),
                    );
                } else {
                    reject(&events, &frame);
                }
                continue;
            }
            let Some(id) = frame["id"].as_u64() else {
                continue;
            };
            let generation = frame["epoch"].as_u64().unwrap_or(0);
            let admitted = if frame["cancel"] == true {
                link.cancel(generation, id)
            } else {
                link.admit(
                    generation,
                    id,
                    frame["method"].as_str().unwrap_or_default(),
                    mutation(&frame),
                )
            };
            if !admitted {
                reject(&events, &frame);
                continue;
            }
            let mut wire = frame;
            wire.as_object_mut().unwrap().remove("epoch");
            if socket.send(&wire.to_string()).is_err() {
                break;
            }
        }
        drop(socket);
        let interrupted = link.close();
        let _ = events.unbounded_send(
            json!({"connection":"retrying","interrupted":interrupted,"paired":paired}),
        );
        if application_closed {
            return;
        }
        if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
            return;
        }
        attempt = attempt.saturating_add(1);
    }
}
