#[cfg(target_family = "wasm")]
mod attachments;
#[cfg(target_family = "wasm")]
mod audio;
#[cfg(target_family = "wasm")]
mod connection;
#[cfg(target_family = "wasm")]
mod drafts;
#[cfg(target_family = "wasm")]
mod socket;
#[cfg(not(target_family = "wasm"))]
fn main() {
    eprintln!("Build this target with scripts/build-web.sh");
}

#[cfg(all(target_family = "wasm", feature = "browser-input-fixture"))]
mod input_fixture;

#[cfg(target_family = "wasm")]
mod browser {
    use futures::channel::mpsc;
    use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use wasm_bindgen::{JsCast, closure::Closure};
    use wasm_bindgen_futures::{JsFuture, spawn_local};
    use web_sys::{Request, RequestInit, Response};
    use zuse_viewport::{Transport, WorkspaceView};

    thread_local! {
        // Browser event loops return immediately; keep GPUI's application owner
        // alive so window callbacks and resize/frame observers remain valid.
        static APPLICATION: RefCell<Option<gpui::ApplicationHandle>> = const { RefCell::new(None) };
    }

    pub(super) async fn unlock() -> Result<(), wasm_bindgen::JsValue> {
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
        crate::audio::install();
        // Opening an invitation in an existing tab is a fragment navigation,
        // so import it through the same fresh connection path as a cold load.
        let import = Closure::<dyn FnMut()>::new(|| {
            let window = web_sys::window().unwrap();
            if window
                .location()
                .hash()
                .ok()
                .is_some_and(|hash| hash.starts_with("#pair=") || hash.starts_with("#token="))
            {
                let _ = window.location().reload();
            }
        });
        web_sys::window()
            .unwrap()
            .add_event_listener_with_callback("hashchange", import.as_ref().unchecked_ref())
            .expect("install invitation import");
        import.forget();
        gpui_platform::web_init();
        let (requests, outbound) = mpsc::unbounded::<Value>();
        let (events, inbound) = mpsc::unbounded::<Value>();
        if let Ok(Some(storage)) = web_sys::window().unwrap().local_storage() {
            if let Ok(Some(value)) = storage.get_item("zuse.workspace.preferences.v1") {
                if value.len() <= 64 * 1024 {
                    if let Ok(value) = serde_json::from_str::<Value>(&value) {
                        let _ = events.unbounded_send(json!({"preferences":value}));
                    }
                }
            }
        }
        spawn_local(super::connection::run(outbound, events));
        let application = gpui_platform::application()
            .with_assets(zuse_viewport::assets::Assets)
            .run_embedded(move |cx: &mut App| {
                zuse_viewport::initialize(cx);
                let bounds = Bounds::centered(None, size(px(1320.), px(880.)), cx);
                let workspace = cx
                    .open_window(
                        WindowOptions {
                            window_bounds: Some(WindowBounds::Windowed(bounds)),
                            ..Default::default()
                        },
                        move |window, cx| {
                            cx.new(|cx| {
                                WorkspaceView::new(
                                    Transport {
                                        requests,
                                        events: inbound,
                                    },
                                    window,
                                    cx,
                                )
                            })
                        },
                    )
                    .expect("open browser workspace");
                crate::attachments::install_paste(workspace, cx.to_async());
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
    #[cfg(feature = "browser-input-fixture")]
    input_fixture::run();
    #[cfg(not(feature = "browser-input-fixture"))]
    browser::run();
}
