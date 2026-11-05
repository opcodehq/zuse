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
enum SessionCheck {
    Ready { changed: bool },
    Denied,
    Busy,
}
async fn auth_status() -> Result<SessionCheck, ()> {
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
    let changed = response
        .headers()
        .get("x-zuse-browser-session-changed")
        .ok()
        .flatten()
        .as_deref()
        == Some("1");
    match response.status() {
        200..=299 | 404 | 405 => Ok(SessionCheck::Ready { changed }),
        401 | 403 => Ok(SessionCheck::Denied),
        429 => Ok(SessionCheck::Busy),
        _ => Err(()),
    }
}
fn local_action(frame: &Value, events: &Sender) -> bool {
    if frame["ui"] == "hostedSessionRevoked" {
        crate::history_cache::signed_out();
        return true;
    }
    if crate::account_connection::action(frame, events) {
        return true;
    }
    if crate::browser_frame::handle(frame) {
        return true;
    }
    if crate::voice_media::handle(frame) {
        return true;
    }
    if crate::cloud_upload::handle(frame) {
        return true;
    }
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
        let mut result = json!({"attachment":{"error":"Reconnect to the engine before uploading files."},"generation":frame["generation"],"epoch":frame["epoch"],"uploadRejected":true});
        if frame.get("cloudViewport").is_some() {
            result["cloudViewport"] = frame["cloudViewport"].clone();
            result["epoch"] = frame["cloudEpoch"].clone();
        }
        let _ = events.unbounded_send(result);
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
// Keep navigation, draft storage and explicit rejection responsive while a
// controller chain is authenticating. Unsent workspace actions are not queued.
async fn connecting<T>(
    work: impl std::future::Future<Output = T>,
    outbound: &mut mpsc::UnboundedReceiver<Value>,
    events: &Sender,
) -> Option<T> {
    let work = work.fuse();
    futures::pin_mut!(work);
    loop {
        futures::select_biased! {
            frame=outbound.next().fuse()=>match frame{Some(frame)=>{if !local_action(&frame,events){reject(events,&frame);}},None=>return None},
            value=work=>return Some(value),
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
    crate::voice_media::install(events.clone());
    let account_target = match crate::account_connection::selected() {
        Ok(target) => target,
        Err(message) => {
            let _ = events.unbounded_send(
                json!({"connection":"accountRequired","message":message,"accountRemote":true}),
            );
            while wait_offline(8000, &mut outbound, &events).await {}
            return;
        }
    };
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
    let local = account_target.is_none()
        && !paired
        && matches!(
            location.hostname().unwrap_or_default().as_str(),
            "localhost" | "127.0.0.1" | "[::1]" | "::1"
        );
    crate::browser_frame::install(events.clone(), local);
    let _ = events.unbounded_send(json!({"browserLocal":local,"paired":paired && account_target.is_none(),"accountRemote":account_target.is_some(),"pairingOrigin":target.as_ref().map(|target|target.origin().to_owned()).unwrap_or_else(||location.origin().unwrap_or_default())}));
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
                Ok(SessionCheck::Denied) => {
                    crate::history_cache::signed_out();
                    let _ = events.unbounded_send(
                        json!({"connection":"authRequired","hostedSessionRevoked":true}),
                    );
                    if !wait_offline(8_000, &mut outbound, &events).await {
                        return;
                    }
                    continue;
                }
                Ok(SessionCheck::Busy) => {
                    let _ = events
                        .unbounded_send(json!({"connection":"retrying","controllerBusy":true}));
                    if !wait_offline(3_000, &mut outbound, &events).await {
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
                Ok(SessionCheck::Ready { changed }) => {
                    if changed {
                        // Only the controller's authoritative session replacement
                        // revokes this actor's cached pages; a network retry does not.
                        crate::history_cache::signed_out();
                        let _ = events.unbounded_send(
                            json!({"connection":"authRequired","hostedSessionRevoked":true}),
                        );
                    }
                }
            }
        }
        let controller = if let Some(account) = account_target.clone() {
            match connecting(
                crate::account_connection::Controller::open(account, &url, target.as_ref()),
                &mut outbound,
                &events,
            )
            .await
            {
                Some(Ok(controller)) => Some(controller),
                None => return,
                Some(Err(message)) => {
                    let _ = events.unbounded_send(
                        json!({"connection":"retrying","accountRemote":true,"message":message}),
                    );
                    if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
                        return;
                    }
                    attempt = attempt.saturating_add(1);
                    continue;
                }
            }
        } else {
            None
        };
        let establish = async {
            match &controller {
                Some(controller) => {
                    super::socket::Socket::connect_account(controller.clone()).await
                }
                None => {
                    super::socket::Socket::connect(
                        &url,
                        target.as_ref().map(PairedTarget::invitation),
                    )
                    .await
                }
            }
        };
        let Some(socket) = connecting(establish, &mut outbound, &events).await else {
            return;
        };
        let mut socket = match socket {
            Ok(socket) => socket,
            Err(message) => {
                let blocked = controller.as_ref().is_some_and(|c| c.is_blocked());
                let _=events.unbounded_send(json!({"connection":if blocked {"accountRequired"}else{"retrying"},"paired":paired && account_target.is_none(),"message":message}));
                if blocked {
                    while wait_offline(8000, &mut outbound, &events).await {}
                    return;
                }
                if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
                    return;
                }
                attempt = attempt.saturating_add(1);
                continue;
            }
        };
        if let Some(controller) = &controller {
            // Verify the actual canonical engine before any workspace read or write.
            let Some(verified) = connecting(
                socket.verify_environment(&controller.target.environment),
                &mut outbound,
                &events,
            )
            .await
            else {
                return;
            };
            if let Err(message) = verified {
                let _ = events
                    .unbounded_send(json!({"connection":"accountRequired","message":message}));
                drop(socket);
                while wait_offline(8000, &mut outbound, &events).await {}
                return;
            }
            let origin = socket.account_origin().unwrap_or_default();
            let _ = events.unbounded_send(
                json!({"pairingOrigin":origin,"accountRemote":true,"paired":false}),
            );
            controller.remember(&origin).await;
        }
        let upload_target = controller
            .clone()
            .map(crate::attachments::UploadTarget::Account)
            .or_else(|| target.clone().map(crate::attachments::UploadTarget::Paired));
        let epoch = link.open();
        crate::history_cache::opened(
            socket
                .account_origin()
                .unwrap_or_else(|| location.origin().unwrap_or_default()),
        );
        attempt = 0;
        let _ =
            events.unbounded_send(json!({"connection":"connected","epoch":epoch,"paired":paired && account_target.is_none(),"accountRemote":account_target.is_some()}));
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
                        if let Ok(mut frame) = serde_json::from_str::<Value>(line) {
                            if let Some(id) = frame["id"].as_u64() {
                                let terminal = frame.get("ok").is_some()
                                    || frame.get("err").is_some()
                                    || frame["done"] == true;
                                if link.receive(epoch, id, terminal) {
                                    crate::history_cache::incoming(&mut frame);
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
                    let cloud = crate::cloud_upload::Target::from_frame(&frame);
                    if frame.get("cloudViewport").is_some() && cloud.is_none() {
                        reject(&events, &frame);
                        continue;
                    }
                    let upload_target = cloud
                        .map(crate::attachments::UploadTarget::Cloud)
                        .or_else(|| upload_target.clone());
                    if frame["ui"] == "pasteAttachments" {
                        crate::attachments::pasted(&frame, events.clone(), upload_target.clone());
                        continue;
                    }
                    crate::attachments::pick_attachments(
                        events.clone(),
                        frame["generation"].as_u64().unwrap_or_default(),
                        epoch,
                        upload_target.clone(),
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
            if crate::history_cache::outgoing(&frame, &events).await {
                link.receive(epoch, id, true);
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
        let blocked = controller.as_ref().is_some_and(|c| c.is_blocked());
        let _=events.unbounded_send(json!({"connection":if blocked {"accountRequired"}else{"retrying"},"interrupted":interrupted,"paired":paired && account_target.is_none(),"accountRemote":account_target.is_some(),"message":if blocked {Some("The controller account changed or computer access was rejected. Return to the controller and select it again.")}else{None}}));
        if blocked {
            while wait_offline(8000, &mut outbound, &events).await {}
            return;
        }
        if application_closed {
            return;
        }
        if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
            return;
        }
        attempt = attempt.saturating_add(1);
    }
}
