#[cfg(not(target_family = "wasm"))]
fn main() {
    eprintln!("Build this target with scripts/build-web.sh");
}

#[cfg(target_family = "wasm")]
mod browser {
    use futures::{
        StreamExt,
        channel::{mpsc, oneshot},
    };
    use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
    use serde_json::{Value, json};
    use std::{cell::RefCell, rc::Rc};
    use wasm_bindgen::{JsCast, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{MessageEvent, Request, RequestInit, Response, WebSocket};
    use zuse_viewport::{Transport, WorkspaceView};

    thread_local! {
        // Browser event loops return immediately; keep GPUI's application owner
        // alive so window callbacks and resize/frame observers remain valid.
        static APPLICATION: RefCell<Option<gpui::ApplicationHandle>> = const { RefCell::new(None) };
    }

    async fn unlock() -> Result<(), wasm_bindgen::JsValue> {
        let window = web_sys::window().unwrap();
        let hash = window.location().hash()?;
        let Some(token) = hash.strip_prefix("#token=") else {
            return Ok(());
        };
        let init = RequestInit::new();
        init.set_method("POST");
        init.set_credentials(web_sys::RequestCredentials::SameOrigin);
        init.set_body(&wasm_bindgen::JsValue::from_str(
            &json!({"token":token}).to_string(),
        ));
        let request = Request::new_with_str_and_init("/api/session", &init)?;
        request.headers().set("Content-Type", "application/json")?;
        let response: Response = JsFuture::from(window.fetch_with_request(&request))
            .await?
            .dyn_into()?;
        if !response.ok() {
            return Err(wasm_bindgen::JsValue::from_str(
                "Engine authentication failed. Open the access link printed by zuse-server.",
            ));
        }
        window
            .history()?
            .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/"))?;
        Ok(())
    }

    pub fn run() {
        gpui_platform::web_init();
        let (requests, mut outbound) = mpsc::unbounded::<Value>();
        let (events, inbound) = mpsc::unbounded::<Value>();
        spawn_local(async move {
            if let Err(error) = unlock().await {
                let _ = events.unbounded_send(json!({"connection":error.as_string().unwrap_or_else(||"Authentication failed".into())}));
                return;
            }
            let location = web_sys::window().unwrap().location();
            let scheme = if location.protocol().unwrap_or_default() == "https:" {
                "wss"
            } else {
                "ws"
            };
            let url = format!("{scheme}://{}/api/rpc", location.host().unwrap_or_default());
            let socket = match WebSocket::new(&url) {
                Ok(socket) => socket,
                Err(_) => {
                    let _ = events
                        .unbounded_send(json!({"connection":"Unable to open engine connection"}));
                    return;
                }
            };
            let (ready_tx, ready_rx) = oneshot::channel();
            let ready_tx = Rc::new(RefCell::new(Some(ready_tx)));
            let open = Closure::<dyn FnMut()>::new(move || {
                if let Some(tx) = ready_tx.borrow_mut().take() {
                    let _ = tx.send(());
                }
            });
            socket.set_onopen(Some(open.as_ref().unchecked_ref()));
            let rx_events = events.clone();
            let message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                if let Some(text) = event.data().as_string() {
                    for line in text.lines() {
                        if let Ok(value) = serde_json::from_str(line) {
                            let _ = rx_events.unbounded_send(value);
                        }
                    }
                }
            });
            socket.set_onmessage(Some(message.as_ref().unchecked_ref()));
            let close_events = events.clone();
            let close = Closure::<dyn FnMut()>::new(move || {
                let _=close_events.unbounded_send(json!({"connection":"Disconnected. Open an authenticated engine link to reconnect."}));
            });
            socket.set_onclose(Some(close.as_ref().unchecked_ref()));
            let error_events = events.clone();
            let error = Closure::<dyn FnMut()>::new(move || {
                let _=error_events.unbounded_send(json!({"connection":"Connection failed. Open the access link printed by zuse-server."}));
            });
            socket.set_onerror(Some(error.as_ref().unchecked_ref()));
            if ready_rx.await.is_ok() {
                while let Some(frame) = outbound.next().await {
                    if socket.send_with_str(&frame.to_string()).is_err() {
                        break;
                    }
                }
            }
            socket.set_onopen(None);
            socket.set_onmessage(None);
            socket.set_onclose(None);
            socket.set_onerror(None);
            let _ = socket.close();
        });
        let application = gpui_platform::application().run_embedded(move |cx: &mut App| {
            zuse_viewport::initialize(cx);
            let bounds = Bounds::centered(None, size(px(1320.), px(880.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |_, cx| {
                    cx.new(|cx| {
                        WorkspaceView::new(
                            Transport {
                                requests,
                                events: inbound,
                            },
                            cx,
                        )
                    })
                },
            )
            .expect("open browser workspace");
            if let Some(boot) = web_sys::window()
                .unwrap()
                .document()
                .and_then(|d| d.get_element_by_id("boot"))
            {
                boot.remove();
            }
            cx.activate(true);
        });
        APPLICATION.with(|owner| *owner.borrow_mut() = Some(application));
    }
}

#[cfg(target_family = "wasm")]
fn main() {
    browser::run();
}
