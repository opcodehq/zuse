//! Native v2 cloud windows use the same GPUI viewport as web. The controller
//! owns auth; this adapter only multiplexes RPC and native file/draft actions.
use super::*;
use crate::state::CloudTarget;
use futures::{StreamExt, channel::mpsc};
use gpui::{ClipboardEntry, PathPromptOptions, Window};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf};
use zuse_protocol::methods;
mod preferences;

pub(crate) fn open_setup(
    controller: crate::state::EngineHandle,
    subject: String,
    organization: Option<String>,
    cx: &mut gpui::App,
) {
    open_draft(controller, subject, organization, None, cx)
}
pub(crate) fn open_project(
    controller: crate::state::EngineHandle,
    subject: String,
    organization: Option<String>,
    project: Value,
    cx: &mut gpui::App,
) {
    open_draft(controller, subject, organization, Some(project), cx)
}
fn open_draft(
    controller: crate::state::EngineHandle,
    subject: String,
    organization: Option<String>,
    project: Option<Value>,
    cx: &mut gpui::App,
) {
    let (bounds, display_id) = crate::restored_main_window_bounds(cx);
    let target = CloudTarget {
        controller,
        subject,
        organization,
        // A draft storage namespace, never a provider/computer identity.
        workspace: project
            .as_ref()
            .and_then(|p| p["projectId"].as_str())
            .map(|id| format!("project:{id}"))
            .unwrap_or_else(|| "setup".into()),
        initial_session: String::new(),
    };
    let origin = std::env::var("ZUSE_CLOUD_API_URL")
        .unwrap_or_else(|_| zuse_engine::cloud::DEFAULT_API_URL.into());
    let _ = cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            display_id,
            window_min_size: Some(gpui::size(px(900.), px(600.))),
            ..Default::default()
        },
        |window, cx| {
            window.set_rem_size(px(crate::typography::font_size(cx).pixels()));
            crate::appearance::observe_window(window, cx).detach();
            cx.new(|cx| {
                Shared::new(
                    target,
                    json!({"apiOrigin":origin,"project":project}),
                    window,
                    cx,
                )
            })
        },
    );
    cx.activate(true);
}

pub(super) struct Shared {
    view: Entity<zuse_viewport::WorkspaceView>,
    target: CloudTarget,
    events: mpsc::UnboundedSender<Value>,
    calls: HashMap<u64, Task<()>>,
    picker: Option<Task<()>>,
    pump: Option<Task<()>>,
    account: Option<Task<()>>,
    revoked: bool,
    drafts: DraftStore,
    preferences: preferences::Store,
}
impl Shared {
    pub(super) fn new(
        target: CloudTarget,
        seed: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        zuse_viewport::initialize(cx);
        let (requests, mut outgoing) = mpsc::unbounded::<Value>();
        let (events, incoming) = mpsc::unbounded();
        let view = cx.new(|cx| {
            zuse_viewport::WorkspaceView::new(
                zuse_viewport::Transport {
                    requests,
                    events: incoming,
                },
                window,
                cx,
            )
        });
        let row = seed["workspace"].clone();
        let has_workspace = row["workspaceId"].is_string();
        if has_workspace {
            view.update(cx, |v, _| {
                v.open_cloud_computer(
                    target.subject.clone(),
                    target.organization.clone(),
                    row,
                    seed.clone(),
                )
            });
        } else {
            view.update(cx, |v, cx| {
                v.open_cloud_setup(
                    target.subject.clone(),
                    target.organization.clone(),
                    seed["apiOrigin"].as_str().map(str::to_owned),
                    cx,
                );
                if seed["project"]["projectId"].is_string() {
                    v.start_cloud_project_session(seed["project"].clone(), window, cx);
                }
            });
        }
        if has_workspace {
            let _ = events.unbounded_send(json!({"connection":"connected","epoch":1}));
        }
        // An account-only draft uses controller HTTP; it has no local engine
        // identity to verify and does not need an execution connection.
        let mut this = Self {
            view,
            target: target.clone(),
            events,
            calls: HashMap::new(),
            picker: None,
            pump: None,
            account: None,
            revoked: false,
            drafts: DraftStore::new(&target, &seed),
            preferences: preferences::Store::new(&target, &seed),
        };
        {
            let store = this.preferences.clone();
            let events = this.events.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Some(value) = store.load() {
                        let _ = events.unbounded_send(json!({"preferences":value}));
                    }
                })
                .detach();
        }
        this.pump = Some(cx.spawn(async move |this, cx| {
            while let Some(frame) = outgoing.next().await {
                if this
                    .update(cx, |this, cx| this.dispatch(frame, cx))
                    .is_err()
                {
                    return;
                }
            }
        }));
        this.account = Some(cx.spawn(async move |this, cx| {
            let mut stream = match target
                .controller
                .client()
                .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
            {
                Ok(s) => s,
                Err(_) => {
                    let _ = this.update(cx, |this, cx| this.revoke(cx));
                    return;
                }
            };
            while let Some(value) = stream.recv().await {
                if value["user"]["id"].as_str() != Some(target.subject.as_str())
                    || value["orgId"].as_str() != target.organization.as_deref()
                {
                    let _ = this.update(cx, |this, cx| this.revoke(cx));
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| this.revoke(cx));
        }));
        this
    }
    fn revoke(&mut self, cx: &mut Context<Self>) {
        self.revoked = true;
        self.calls.clear();
        self.picker = None;
        self.drafts.purge();
        self.preferences.purge();
        cx.notify();
    }
    fn dispatch(&mut self, frame: Value, cx: &mut Context<Self>) {
        if self.revoked {
            return;
        }
        if let Some(action) = frame["ui"].as_str() {
            match action {
                "loadDrafts" | "saveDrafts" => {
                    let store = self.drafts.clone();
                    let events = self.events.clone();
                    let revision = if action == "saveDrafts" {
                        Some(
                            store
                                .latest
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                                + 1,
                        )
                    } else {
                        None
                    };
                    cx.background_executor()
                        .spawn(async move {
                            let value = store.handle(&frame, revision);
                            let _ = events.unbounded_send(json!({"browserDrafts":value}));
                        })
                        .detach();
                }
                "savePreferences" => {
                    let store = self.preferences.clone();
                    let value = frame["value"].clone();
                    let revision = store.next();
                    cx.background_executor()
                        .spawn(async move {
                            store.save(value, revision);
                        })
                        .detach();
                }
                "pickAttachments" => self.pick(frame, cx),
                "pasteAttachments" => {
                    let mut staged = Vec::new();
                    let mut paths = Vec::new();
                    if let Some(item) = cx.read_from_clipboard() {
                        for entry in item.entries {
                            match entry {
                                ClipboardEntry::Image(image) => {
                                    staged.push(crate::attachments::stage_clipboard_image(image))
                                }
                                ClipboardEntry::ExternalPaths(files) => {
                                    paths.extend(files.paths().iter().cloned())
                                }
                                _ => {}
                            }
                        }
                    }
                    self.upload(frame, staged, paths, cx);
                }
                "browserFrame" => {
                    // Native browser embedding is handled by the desktop browser
                    // adapter, never by silently opening an unauthenticated URL.
                    let _=self.events.unbounded_send(json!({"browserFrame":{"kind":"capability","supported":false,"generation":frame["generation"]}}));
                }
                _ => {}
            }
            return;
        }
        let Some(id) = frame["id"].as_u64() else {
            return;
        };
        self.calls.retain(|_, task| !task.is_ready());
        if frame["cancel"] == true {
            self.calls.remove(&id);
            return;
        }
        let Some(method) = frame["method"].as_str().map(str::to_owned) else {
            return;
        };
        let controller = self.target.controller.clone();
        let events = self.events.clone();
        let mut params = frame["params"].clone();
        if zuse_engine::cloud::handles(&method) {
            if params
                .get("expectedAccountId")
                .is_some_and(|v| v.as_str() != Some(self.target.subject.as_str()))
                || params
                    .get("expectedOrganizationId")
                    .is_some_and(|v| v.as_str() != self.target.organization.as_deref())
            {
                let _ = events.unbounded_send(
                    json!({"id":id,"err":"The native cloud scope changed. Reopen this computer."}),
                );
                return;
            }
            params["expectedAccountId"] = json!(self.target.subject);
            params["expectedOrganizationId"] = json!(self.target.organization);
        }
        self.calls.insert(
            id,
            cx.spawn(async move |_, _| {
                let mut stream = match controller.client().forward(&method, params).await {
                    Ok(s) => s,
                    Err(error) => {
                        let _ = events.unbounded_send(json!({"id":id,"err":error.to_string()}));
                        return;
                    }
                };
                while let Some(mut value) = stream.recv().await {
                    value.id = id;
                    if let Ok(value) = serde_json::to_value(value) {
                        if events.unbounded_send(value).is_err() {
                            return;
                        }
                    }
                }
            }),
        );
    }
    fn pick(&mut self, frame: Value, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            return;
        }
        let reply = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        self.picker = Some(cx.spawn(async move |this, cx| {
            let paths = match reply.await {
                Ok(Ok(Some(paths))) => paths,
                _ => Vec::new(),
            };
            let _ = this.update(cx, |this, cx| {
                this.picker = None;
                this.upload(frame, Vec::new(), paths, cx);
            });
        }));
    }
    fn upload(
        &mut self,
        frame: Value,
        mut staged: Vec<crate::attachments::StagedAttachment>,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.picker.is_some() {
            return;
        }
        let target = self.target.clone();
        let events = self.events.clone();
        self.picker = Some(cx.spawn(async move |this, cx| {
            let stage = cx
                .background_executor()
                .spawn(async move {
                    for path in paths {
                        staged.push(crate::attachments::stage_file(&path)?);
                    }
                    if staged.len() > zuse_protocol::cloud_assets::MAX_ASSETS {
                        return Err("Attach up to eight files.".to_owned());
                    }
                    Ok::<_, String>(staged)
                })
                .await;
            let result = match stage {
                Ok(files) => upload_files(&target, &frame, &events, files).await,
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                let _ = events.unbounded_send(decorate(
                    &frame,
                    json!({"attachment":{"error":error},"generation":frame["generation"]}),
                ));
            }
            let _ = events.unbounded_send(decorate(
                &frame,
                json!({"uploading":0,"generation":frame["generation"]}),
            ));
            let _ = this.update(cx, |this, cx| {
                this.picker = None;
                cx.notify();
            });
        }));
    }
}
fn decorate(source: &Value, mut value: Value) -> Value {
    value["epoch"] = source
        .get("cloudEpoch")
        .or_else(|| source.get("epoch"))
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(viewport) = source.get("cloudViewport") {
        value["cloudViewport"] = viewport.clone();
    }
    value
}
async fn upload_files(
    target: &CloudTarget,
    frame: &Value,
    events: &mpsc::UnboundedSender<Value>,
    files: Vec<crate::attachments::StagedAttachment>,
) -> Result<(), String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let _ = events.unbounded_send(decorate(
        frame,
        json!({"uploading":files.len(),"generation":frame["generation"]}),
    ));
    for file in files {
        if file.bytes().is_empty() || file.bytes().len() > zuse_protocol::cloud_assets::MAX_BYTES {
            return Err("Cloud attachments must contain at most20MB.".into());
        }
        let workspace = frame["cloudWorkspaceId"].as_str();
        for (seq, part) in file
            .bytes()
            .chunks(zuse_protocol::cloud_assets::CHUNK_BYTES)
            .enumerate()
        {
            let params = json!({"uploadId":file.id,"seq":seq,"data":STANDARD.encode(part)});
            if let Some(workspace) = workspace {
                target.controller.client().call(methods::CLOUD_TASK_ASSET,json!({"workspaceId":workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization,"method":methods::UPLOAD_CHUNK,"params":params})).await.map_err(|e|e.to_string())?;
            } else {
                target
                    .controller
                    .client()
                    .call(methods::UPLOAD_CHUNK, params)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        let params = json!({"uploadId":file.id,"fileName":file.name});
        let receipt = if let Some(workspace) = workspace {
            target.controller.client().call(methods::CLOUD_TASK_ASSET,json!({"workspaceId":workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization,"method":methods::UPLOAD_COMMIT,"params":params})).await.map_err(|e|e.to_string())?["data"].clone()
        } else {
            target
                .controller
                .client()
                .call(methods::UPLOAD_COMMIT, params)
                .await
                .map_err(|e| e.to_string())?
        };
        let path = receipt["path"]
            .as_str()
            .ok_or_else(|| "Upload ended without a committed attachment.".to_owned())?;
        let _=events.unbounded_send(decorate(frame,json!({"attachment":{"path":path,"name":file.name,"fileName":file.name,"uploadId":file.id},"generation":frame["generation"]})));
    }
    Ok(())
}
impl Render for Shared {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.revoked {
            return gpui::div()
                .size_full()
                .p_6()
                .child("Cloud access changed. Reopen this computer from its account.")
                .into_any_element();
        }
        zuse_viewport::palette::set_light(crate::theme::Theme::of(cx).appearance.is_light());
        gpui::div()
            .size_full()
            .child(self.view.clone())
            .into_any_element()
    }
}

/// Draft copies are bounded, scoped local data. They never replay a command.
#[derive(Clone)]
struct DraftStore {
    root: Option<PathBuf>,
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latest: std::sync::Arc<std::sync::atomic::AtomicU64>,
    gate: std::sync::Arc<std::sync::Mutex<()>>,
}
impl DraftStore {
    fn new(target: &CloudTarget, seed: &Value) -> Self {
        use sha2::{Digest, Sha256};
        let root = seed["apiOrigin"]
            .as_str()
            .filter(|s| s.starts_with("https://") || s.starts_with("http://127.0.0.1"))
            .and_then(|origin| {
                std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home)
                        .join(".zuse/cache/cloud-view-drafts")
                        .join(format!(
                            "{:x}",
                            Sha256::digest(
                                json!([
                                    origin,
                                    target.subject,
                                    target.organization,
                                    target.workspace
                                ])
                                .to_string()
                                .as_bytes()
                            )
                        ))
                })
            });
        Self {
            root,
            active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            latest: Default::default(),
            gate: Default::default(),
        }
    }
    fn handle(&self, frame: &Value, revision: Option<u64>) -> Value {
        use sha2::{Digest, Sha256};
        use zuse_protocol::browser_drafts::{DraftJournal, MAX_JOURNAL_BYTES};
        let device = frame["deviceId"].as_str().unwrap_or("");
        let result = (|| -> Result<Value, String> {
            let _gate = self
                .gate
                .lock()
                .map_err(|_| "Native draft storage is unavailable.")?;
            if !self.active.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("Cloud draft scope was revoked.".into());
            }
            if revision.is_some_and(|r| r != self.latest.load(std::sync::atomic::Ordering::SeqCst))
            {
                return Ok(json!({"deviceId":device,"saved":false}));
            }
            let root = self.root.as_ref().ok_or_else(|| {
                "Native draft storage is unavailable for this API scope.".to_owned()
            })?;
            std::fs::create_dir_all(root)
                .map_err(|_| "Native draft directory could not be created.")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| "Native draft directory could not be secured.")?;
            }
            let path = root.join(format!("{:x}.json", Sha256::digest(device.as_bytes())));
            if frame["ui"] == "saveDrafts" {
                let journal =
                    DraftJournal::decode(&frame["value"].to_string()).map_err(str::to_owned)?;
                let bytes = journal.encode().map_err(str::to_owned)?;
                use std::io::Write;
                let temp = root.join(format!("{}.tmp", uuid::Uuid::new_v4()));
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options
                    .open(&temp)
                    .map_err(|_| "Native draft could not be opened.")?;
                file.write_all(bytes.as_bytes())
                    .map_err(|_| "Native draft could not be saved.")?;
                file.sync_all()
                    .map_err(|_| "Native draft could not be flushed.")?;
                if !self.active.load(std::sync::atomic::Ordering::SeqCst)
                    || revision
                        .is_some_and(|r| r != self.latest.load(std::sync::atomic::Ordering::SeqCst))
                {
                    let _ = std::fs::remove_file(temp);
                    return Ok(json!({"deviceId":device,"saved":false}));
                }
                std::fs::rename(&temp, &path)
                    .map_err(|_| "Native draft could not be committed.")?;
                return Ok(json!({"deviceId":device,"saved":true}));
            }
            let journal = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.is_file()
                        || metadata.file_type().is_symlink()
                        || metadata.len() > MAX_JOURNAL_BYTES as u64
                    {
                        return Err("Native draft file is invalid.".into());
                    }
                    let text = std::fs::read_to_string(path)
                        .map_err(|_| "Native draft could not be read.")?;
                    DraftJournal::decode(&text).map_err(str::to_owned)?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    DraftJournal::default()
                }
                Err(_) => return Err("Native draft could not be read.".into()),
            };
            Ok(json!({"deviceId":device,"journal":journal}))
        })();
        result.unwrap_or_else(|error| json!({"deviceId":device,"error":error}))
    }
    fn purge(&self) {
        self.active
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let Ok(_guard) = self.gate.lock() else {
            return;
        };
        if let Some(root) = &self.root {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    if entry.path().extension().is_some_and(|e| e == "json")
                        && entry.file_type().is_ok_and(|t| t.is_file())
                    {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

#[cfg(feature = "appshots-fixture")]
pub(crate) fn fixture(
    client: zuse_rpc::RpcClient,
    seed: Value,
    cx: &mut gpui::App,
) -> gpui::AnyWindowHandle {
    let target = CloudTarget {
        controller: crate::state::EngineHandle::from_test_client(client),
        subject: "fixture-owner".into(),
        organization: None,
        workspace: "fixture-computer".into(),
        initial_session: "fixture-session".into(),
    };
    cx.open_window(
        gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                None,
                gpui::size(px(1160.), px(850.)),
                cx,
            ))),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Shared::new(target, seed, window, cx)),
    )
    .unwrap()
    .into()
}
