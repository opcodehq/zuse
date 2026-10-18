//! Browser files share one upload path, scoped to their admitting composer and host.
use futures::channel::mpsc;
use gpui::{AsyncApp, WindowHandle};
use serde_json::{Value, json};
use std::{cell::RefCell, collections::HashMap};
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Request, RequestInit, Response};
use zuse_viewport::WorkspaceView;

type Sender = mpsc::UnboundedSender<Value>;
#[derive(Clone)]
pub(super) enum UploadTarget {
    Paired(zuse_remote::PairedTarget),
    Account(crate::account_connection::Controller),
}
#[derive(Default)]
struct PendingFiles {
    next: u64,
    batches: HashMap<u64, Vec<web_sys::File>>,
}
thread_local! { static PENDING: RefCell<PendingFiles> = RefCell::default(); }
fn retain(files: Vec<web_sys::File>) -> u64 {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        pending.next = pending.next.wrapping_add(1);
        let ticket = pending.next;
        pending.batches.insert(ticket, files);
        ticket
    })
}
fn take(ticket: u64) -> Option<Vec<web_sys::File>> {
    PENDING.with(|pending| pending.borrow_mut().batches.remove(&ticket))
}
pub(super) fn discard(frame: &Value) {
    if frame["ui"] == "pasteAttachments" {
        if let Some(ticket) = frame["ticket"].as_u64() {
            take(ticket);
        }
    }
}
pub(super) fn pasted(frame: &Value, events: Sender, target: Option<UploadTarget>) {
    if let Some(files) = frame["ticket"].as_u64().and_then(take) {
        upload(
            files,
            events,
            frame["generation"].as_u64().unwrap_or_default(),
            frame["epoch"].as_u64().unwrap_or_default(),
            target,
        );
    }
}

pub(super) fn install_paste(workspace: WindowHandle<WorkspaceView>, mut app: AsyncApp) {
    let paste = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(
        move |event: web_sys::ClipboardEvent| {
            let Some(files) = event.clipboard_data().and_then(|data| data.files()) else {
                return;
            };
            let files: Vec<_> = (0..files.length()).filter_map(|i| files.get(i)).collect();
            if files.is_empty() {
                return;
            }
            let ticket = retain(files);
            let admitted = workspace
                .update(&mut app, |view, window, cx| {
                    view.paste_attachments(ticket, window, cx)
                })
                .unwrap_or(false);
            if admitted {
                event.prevent_default();
                event.stop_immediate_propagation();
            } else {
                take(ticket);
            }
        },
    );
    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .add_event_listener_with_callback_and_bool("paste", paste.as_ref().unchecked_ref(), true)
        .expect("install composer attachment paste");
    paste.forget();
}

pub(super) fn pick_attachments(
    events: Sender,
    generation: u64,
    epoch: u64,
    target: Option<UploadTarget>,
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
        let files = (0..files.length()).filter_map(|i| files.get(i)).collect();
        upload(files, events, generation, epoch, target);
    });
    input.set_onchange(Some(change.unchecked_ref()));
    let _ = input.add_event_listener_with_callback("cancel", change.unchecked_ref());
    input.click();
}

fn upload(
    files: Vec<web_sys::File>,
    events: Sender,
    generation: u64,
    epoch: u64,
    target: Option<UploadTarget>,
) {
    let _ = events
        .unbounded_send(json!({"uploading":files.len(),"generation":generation,"epoch":epoch}));
    for file in files {
        let events = events.clone();
        let target = target.clone();
        spawn_local(async move {
            let name = file.name();
            let mime = file.type_();
            let result = match target {
                Some(target) => upload_remote(file, &target).await,
                None => upload_file(file).await,
            };
            let attachment = match result {
                Ok(path) => json!({"name":name,"mime":mime,"path":path}),
                Err(error) => json!({"error":error}),
            };
            let _ = events.unbounded_send(
                json!({"attachment":attachment,"generation":generation,"epoch":epoch}),
            );
        });
    }
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
    let request = Request::new_with_str_and_init(&format!("/api/attachments?name={name}"), &init)
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

async fn upload_remote(file: web_sys::File, target: &UploadTarget) -> Result<String, String> {
    use base64::Engine as _;
    if file.size() > 32. * 1024. * 1024. {
        return Err("Attachments must be 32 MB or smaller.".into());
    }
    let mut socket = match target {
        UploadTarget::Paired(target) => {
            super::socket::Socket::connect(&target.socket_url(), Some(target.invitation())).await?
        }
        UploadTarget::Account(controller) => {
            super::socket::Socket::connect_account(controller.clone()).await?
        }
    };
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
