//! Browser socket lifetime and request admission. Only the view reopens reads.
use futures::{
    FutureExt, StreamExt,
    channel::{mpsc, oneshot},
};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{MessageEvent, Request, RequestInit, Response, WebSocket};
use zuse_protocol::{
    browser_link::{BrowserLink, retry_delay_ms},
    methods,
};

type Sender = mpsc::UnboundedSender<Value>;
async fn delay(ms: u32) {
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
fn reject(events: &Sender, frame: &Value) {
    if let Some(id) = frame["id"].as_u64() {
        if frame["cancel"] != true {
            let _=events.unbounded_send(json!({"id":id,"err":"The engine is not connected. This request was not sent.","outcomeUnknown":false}));
        }
    } else if frame["ui"] == "pickAttachments" {
        let _=events.unbounded_send(json!({"attachment":{"error":"Reconnect to the engine before uploading files."},"generation":frame["generation"]}));
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
            frame=outbound.next().fuse()=>match frame {Some(frame)=>reject(events,&frame),None=>return false},
        }
    }
}
fn mutation(frame: &Value) -> bool {
    match frame["method"].as_str().unwrap_or_default() {
        methods::CLOUD_ACCOUNT_STATUS
        | methods::LOCAL_DEVICE
        | methods::LIST_HARNESSES
        | methods::WATCH_CHATS
        | methods::WATCH_SESSIONS
        | methods::LIST_REPOS
        | methods::MODEL_CONNECTIONS
        | methods::LIST_MODELS
        | methods::LIST_AGENT_ACCOUNTS
        | methods::WATCH_DOC_MESSAGES
        | methods::WATCH_QUEUE
        | methods::LIST_WORKSPACE_DIRECTORY
        | methods::READ_WORKSPACE_FILE
        | methods::READ_ATTACHMENT_CHUNK
        | methods::GET_CHECKOUT_DIFF
        | methods::GET_CHECKOUT_FILE_DIFF_TEXT
        | methods::FETCH_TOOL_BLOB
        | methods::SUBSCRIBE_TERMINAL
        | methods::INSPECT_RECOVERY => false,
        methods::PLUGINS if frame["params"]["action"] == "list" => false,
        _ => true,
    }
}
pub(super) async fn run(mut outbound: mpsc::UnboundedReceiver<Value>, events: Sender) {
    if let Err(error) = super::browser::unlock().await {
        let _=events.unbounded_send(json!({"connection":"authRequired","message":error.as_string().unwrap_or_else(||"Authentication required".into())}));
    }
    let location = web_sys::window().unwrap().location();
    let local = matches!(
        location.hostname().unwrap_or_default().as_str(),
        "localhost" | "127.0.0.1" | "[::1]" | "::1"
    );
    let _ = events.unbounded_send(json!({"browserLocal":local}));
    let scheme = if location.protocol().unwrap_or_default() == "https:" {
        "wss"
    } else {
        "ws"
    };
    let url = format!("{scheme}://{}/api/rpc", location.host().unwrap_or_default());
    let link = Rc::new(RefCell::new(BrowserLink::default()));
    let mut attempt = 0;
    loop {
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
        let socket = match WebSocket::new(&url) {
            Ok(socket) => socket,
            Err(_) => {
                let _ = events.unbounded_send(json!({"connection":"retrying"}));
                if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
                    return;
                }
                attempt = attempt.saturating_add(1);
                continue;
            }
        };
        let (ready_tx, ready_rx) = oneshot::channel();
        let ready = Rc::new(RefCell::new(Some(ready_tx)));
        let open_ready = ready.clone();
        let open = Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = open_ready.borrow_mut().take() {
                let _ = tx.send(true);
            }
        });
        socket.set_onopen(Some(open.as_ref().unchecked_ref()));
        let (closed_tx, closed_rx) = oneshot::channel();
        let closed = Rc::new(RefCell::new(Some(closed_tx)));
        let close_ready = ready.clone();
        let close_signal = closed.clone();
        let close = Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = close_ready.borrow_mut().take() {
                let _ = tx.send(false);
            }
            if let Some(tx) = close_signal.borrow_mut().take() {
                let _ = tx.send(());
            }
        });
        socket.set_onclose(Some(close.as_ref().unchecked_ref()));
        let error_ready = ready.clone();
        let error_closed = closed.clone();
        let error = Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = error_ready.borrow_mut().take() {
                let _ = tx.send(false);
            }
            if let Some(tx) = error_closed.borrow_mut().take() {
                let _ = tx.send(());
            }
        });
        socket.set_onerror(Some(error.as_ref().unchecked_ref()));
        let connected = futures::select_biased! {ready=ready_rx.fuse()=>ready==Ok(true),_=delay(5_000).fuse()=>false};
        let mut application_closed = false;
        let mut epoch = 0;
        let message_link = link.clone();
        let message_events = events.clone();
        // Installed after opening, before any request can be admitted.
        if connected {
            epoch = link.borrow_mut().open();
            attempt = 0;
        }
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            if let Some(text) = event.data().as_string() {
                for line in text.lines() {
                    if let Ok(frame) = serde_json::from_str::<Value>(line) {
                        if let Some(id) = frame["id"].as_u64() {
                            let terminal = frame.get("ok").is_some()
                                || frame.get("err").is_some()
                                || frame["done"] == true;
                            if message_link.borrow_mut().receive(epoch, id, terminal) {
                                let _ = message_events.unbounded_send(frame);
                            }
                        }
                    }
                }
            }
        });
        socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
        if connected {
            let _ = events.unbounded_send(json!({"connection":"connected","epoch":epoch}));
            let closed = closed_rx.fuse();
            futures::pin_mut!(closed);
            loop {
                let frame = futures::select_biased! {_=closed=>break,frame=outbound.next().fuse()=>match frame{Some(frame)=>frame,None=>{application_closed=true;break;}}};
                if frame["ui"] == "pickAttachments" {
                    if frame["epoch"].as_u64() == Some(epoch) {
                        super::browser::pick_attachments(
                            events.clone(),
                            frame["generation"].as_u64().unwrap_or_default(),
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
                    link.borrow_mut().cancel(generation, id)
                } else {
                    link.borrow_mut().admit(
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
                if socket.send_with_str(&wire.to_string()).is_err() {
                    break;
                }
            }
        }
        socket.set_onopen(None);
        socket.set_onmessage(None);
        socket.set_onclose(None);
        socket.set_onerror(None);
        let _ = socket.close();
        let interrupted = link.borrow_mut().close();
        let _ = events.unbounded_send(json!({"connection":"retrying","interrupted":interrupted}));
        if application_closed {
            return;
        }
        if !wait_offline(retry_delay_ms(attempt), &mut outbound, &events).await {
            return;
        }
        attempt = attempt.saturating_add(1);
    }
}
