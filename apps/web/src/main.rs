#[cfg(target_family = "wasm")]
mod connection;
#[cfg(target_family = "wasm")]
mod socket;
#[cfg(not(target_family = "wasm"))]
fn main() {
    eprintln!("Build this target with scripts/build-web.sh");
}

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

    pub(super) fn pick_attachments(
        events: mpsc::UnboundedSender<Value>,
        generation: u64,
        target: Option<zuse_remote::PairedTarget>,
    ) {
        let document = web_sys::window().unwrap().document().unwrap();
        let input: web_sys::HtmlInputElement =
            document.create_element("input").unwrap().unchecked_into();
        input.set_type("file");
        input.set_multiple(true);
        let _ = input.set_attribute("hidden", "");
        let _ = input.set_attribute("data-zuse-attachments", "");
        let _ = document.body().unwrap().append_child(&input);
        let selected = input.clone();
        let change = Closure::once_into_js(move || {
            selected.remove();
            let Some(files) = selected.files() else {
                return;
            };
            let _ =
                events.unbounded_send(json!({"uploading":files.length(),"generation":generation}));
            for i in 0..files.length() {
                let Some(file) = files.get(i) else {
                    continue;
                };
                let events = events.clone();
                let target = target.clone();
                spawn_local(async move {
                    let name = file.name();
                    let mime = file.type_();
                    let result = match target {
                        Some(target) => upload_paired(file, &target).await,
                        None => upload_file(file).await,
                    };
                    let frame = match result {
                        Ok(path) => {
                            json!({"attachment":{"name":name,"mime":mime,"path":path},"generation":generation})
                        }
                        Err(error) => json!({"attachment":{"error":error},"generation":generation}),
                    };
                    let _ = events.unbounded_send(frame);
                });
            }
        });
        input.set_onchange(Some(change.unchecked_ref()));
        let _ = input.add_event_listener_with_callback("cancel", change.unchecked_ref());
        input.click();
    }

    async fn upload_file(file: web_sys::File) -> Result<String, String> {
        if file.size() > 32. * 1024. * 1024. {
            return Err("Attachments must be 32 MB or smaller.".into());
        }
        let init = RequestInit::new();
        init.set_method("POST");
        init.set_credentials(web_sys::RequestCredentials::SameOrigin);
        init.set_body(file.as_ref());
        let name = js_sys::encode_uri_component(&file.name());
        let request =
            Request::new_with_str_and_init(&format!("/api/attachments?name={name}"), &init)
                .map_err(|_| "Could not prepare upload")?;
        request
            .headers()
            .set("Content-Type", "application/octet-stream")
            .map_err(|_| "Could not prepare upload")?;
        let response: Response =
            JsFuture::from(web_sys::window().unwrap().fetch_with_request(&request))
                .await
                .map_err(|_| "Upload connection failed")?
                .dyn_into()
                .map_err(|_| "Invalid upload response")?;
        if !response.ok() {
            return Err(format!("Upload failed ({})", response.status()));
        }
        let value = JsFuture::from(response.text().map_err(|_| "Invalid upload response")?)
            .await
            .map_err(|_| "Invalid upload response")?;
        let data: Value = serde_json::from_str(&value.as_string().unwrap_or_default())
            .map_err(|_| "Invalid upload response")?;
        data["path"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Attachment did not finish uploading".into())
    }

    async fn upload_paired(
        file: web_sys::File,
        target: &zuse_remote::PairedTarget,
    ) -> Result<String, String> {
        use base64::Engine as _;
        if file.size() > 32. * 1024. * 1024. {
            return Err("Attachments must be 32 MB or smaller.".into());
        }
        let mut socket =
            super::socket::Socket::connect(&target.socket_url(), Some(target.invitation())).await?;
        let upload_id = uuid::Uuid::new_v4().simple().to_string();
        let mut offset = 0usize;
        let mut seq = 0usize;
        let size = file.size() as usize;
        loop {
            let end = (offset + 45_000).min(size);
            let slice = file
                .slice_with_f64_and_f64(offset as f64, end as f64)
                .map_err(|_| "Could not read attachment")?;
            let bytes = JsFuture::from(slice.array_buffer())
                .await
                .map_err(|_| "Could not read attachment")?;
            let data = base64::engine::general_purpose::STANDARD
                .encode(js_sys::Uint8Array::new(&bytes).to_vec());
            paired_upload_call(
                &mut socket,
                seq as u64 + 1,
                zuse_protocol::methods::UPLOAD_CHUNK,
                json!({"uploadId":upload_id,"seq":seq,"data":data}),
            )
            .await?;
            seq += 1;
            offset = end;
            if offset == size {
                break;
            }
        }
        let result = paired_upload_call(
            &mut socket,
            seq as u64 + 1,
            zuse_protocol::methods::UPLOAD_COMMIT,
            json!({"uploadId":upload_id,"fileName":file.name()}),
        )
        .await?;
        result["path"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Attachment did not finish uploading".into())
    }
    async fn paired_upload_call(
        socket: &mut super::socket::Socket,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        use futures::FutureExt;
        socket.send(&json!({"id":id,"method":method,"params":params}).to_string())?;
        let text = futures::select_biased! {
            result = socket.receive().fuse() => result?,
            _ = super::connection::delay(30_000).fuse() => return Err("Upload connection timed out. Try attaching the file again.".into()),
        };
        let reply: Value = serde_json::from_str(&text).map_err(|_| "Invalid upload response")?;
        if reply["id"].as_u64() != Some(id) {
            return Err("Invalid upload response".into());
        }
        if let Some(error) = reply["err"].as_str() {
            return Err(error.to_owned());
        }
        reply
            .get("ok")
            .cloned()
            .ok_or_else(|| "Invalid upload response".into())
    }

    pub fn run() {
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
                cx.open_window(
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
