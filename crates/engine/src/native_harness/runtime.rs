use super::{
    NativeHarness,
    events::Mapper,
    failure,
    host::{self, Application, Notices},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{FutureExt, StreamExt, stream::BoxStream};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tokio::{io::AsyncReadExt, sync::mpsc, task::JoinHandle};
use zuse_harness::{
    Engine, Error, Event, Host, Image, SendOutcome,
    native::RootStore,
    native_tools::{NativeOptions, NativeTools},
    processes::{ProcessOptions, ProcessStatus},
};
use zuse_protocol::{AgentEvent, DoneStatus, HarnessId, RunRequest};
use zuse_providers::{HarnessError, RunControls, RunIdentity};

fn session_id(
    request: &RunRequest,
    device: &str,
    identity: Option<&RunIdentity>,
) -> Result<String, HarnessError> {
    match &request.resume {
        Some(id)
            if id
                .strip_prefix("zuse:")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()) =>
        {
            Ok(id.clone())
        }
        Some(_) => Err(failure(
            "This resume cursor belongs to another provider. Start a Zuse conversation with its transcript context.",
        )),
        None => {
            let id = match identity {
                None => uuid::Uuid::new_v4(),
                Some(identity) => {
                    let key = serde_json::to_vec(&(
                        "zuse.initial-session.v1",
                        device,
                        &identity.chat_id,
                        &identity.message_id,
                    ))
                    .map_err(failure)?;
                    let hash = Sha256::digest(key);
                    let mut bytes = [0u8; 16];
                    bytes.copy_from_slice(&hash[..16]);
                    bytes[6] = (bytes[6] & 0x0f) | 0x80;
                    bytes[8] = (bytes[8] & 0x3f) | 0x80;
                    uuid::Uuid::from_bytes(bytes)
                }
            };
            Ok(format!("zuse:{id}"))
        }
    }
}
async fn checkout_manifest(directory: &Path, cwd: &Path, recovered: bool) -> Result<(), Error> {
    let manifest = directory.join("checkout.json");
    match tokio::fs::read(&manifest).await {
        Ok(bytes) => {
            let saved: PathBuf =
                serde_json::from_slice(&bytes).map_err(|_| Error::InvalidCheckpoint)?;
            if saved != cwd {
                return Err(Error::paused(
                    "The saved Zuse conversation belongs to another checkout.",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !recovered => {
            tokio::fs::write(
                &manifest,
                serde_json::to_vec(cwd).map_err(|_| Error::Storage)?,
            )
            .await
            .map_err(|_| Error::Storage)?;
        }
        Err(_) => return Err(Error::InvalidCheckpoint),
    }
    Ok(())
}
async fn images(paths: &[String]) -> Result<Vec<Image>, HarnessError> {
    if paths.len() > 20 {
        return Err(failure("Too many image attachments"));
    }
    let mut result = vec![];
    let mut total = 0;
    for path in paths {
        let file = tokio::fs::File::open(path).await?;
        let mut bytes = vec![];
        file.take(20 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        total += bytes.len();
        if bytes.len() > 20 * 1024 * 1024 || total > 32 * 1024 * 1024 {
            return Err(failure("Image attachments exceed the inline size limit"));
        }
        let media_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            "image/gif"
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else {
            return Err(failure("Unsupported image attachment"));
        };
        result.push(Image {
            media_type: media_type.into(),
            data: STANDARD.encode(bytes),
        });
    }
    Ok(result)
}
async fn emit(
    tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
    event: Result<AgentEvent, HarnessError>,
    stop: &tokio_util::sync::CancellationToken,
) -> bool {
    tokio::select! {biased;_=stop.cancelled()=>false,result=tx.send(event)=>result.is_ok()}
}
fn send(
    engine: Engine,
    prompt: String,
    images: Vec<Image>,
    tx: mpsc::Sender<Event>,
    admission: tokio_util::sync::CancellationToken,
    delivery_id: Option<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let root = engine.snapshot().await.root_id;
        let result = std::panic::AssertUnwindSafe(async {
            match delivery_id {
                Some(id) => {
                    engine
                        .send_delivery_with_cancel(id, prompt, images, &admission)
                        .await
                }
                None => engine
                    .send_with_cancel(prompt, images, &admission)
                    .await
                    .map(|()| SendOutcome::Started),
            }
        })
        .catch_unwind()
        .await;
        let error = match result {
            Ok(Ok(SendOutcome::Started)) => return,
            Ok(Ok(SendOutcome::AlreadyConsumed)) => {
                "This message was already accepted. Review the conversation before continuing."
                    .into()
            }
            Ok(Err(error)) => error.to_string(),
            Err(_) => "Native session failed. Inspect the durable journal before retrying.".into(),
        };
        let _ = tx
            .send(Event::Paused {
                agent_id: root,
                reason: error,
            })
            .await;
    })
}
pub(super) async fn start(
    harness: NativeHarness,
    mut request: RunRequest,
    mut controls: RunControls,
    identity: Option<RunIdentity>,
) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
    let session = session_id(&request, &harness.device_id, identity.as_ref())?;
    let root = session.clone();
    let cwd = tokio::fs::canonicalize(&request.cwd).await?;
    if !tokio::fs::metadata(&cwd).await?.is_dir() {
        return Err(failure("The session checkout is not a directory"));
    }
    request.cwd = cwd.to_string_lossy().into_owned();
    let stop = controls.interrupt.child_token();
    let store = RootStore::acquire(&harness.profile, &root).map_err(failure)?;
    let saved = store.load().map_err(failure)?;
    if request.resume.is_some() && saved.is_none() {
        return Err(failure(
            "The saved Zuse conversation journal is missing. Restore it before resuming.",
        ));
    }
    checkout_manifest(
        store.directory(),
        &cwd,
        saved.is_some() || request.resume.is_some(),
    )
    .await
    .map_err(failure)?;
    // An interrupted model request can continue from its durable history.
    // A completed response or a deliberate Stop must never become a new turn.
    let resuming_request = identity.as_ref().is_some_and(|identity| {
        saved.as_ref().is_some_and(|state| {
            state
                .consumed_notifications
                .contains_key(&identity.message_id)
                && state.agent(&root).is_ok_and(|agent| {
                    agent.status == zuse_harness::Status::Running
                        && agent.history.last().is_some_and(|message| {
                            message.role != zuse_harness::Role::Assistant
                                || message.has_tool_calls()
                        })
                })
        })
    });
    if let Some(identity) = &identity
        && let Some(saved) = &saved
        && ((saved
            .consumed_notifications
            .contains_key(&identity.message_id)
            && !resuming_request)
            || saved.cancelled_notifications.contains(&identity.message_id))
    {
        let consumed = saved
            .consumed_notifications
            .contains_key(&identity.message_id);
        let lease = controls.execution_lease.take();
        let mut events = vec![AgentEvent::SessionStarted {
            harness: HarnessId::Zuse,
            model: saved.model.clone(),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: session.clone(),
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
        }];
        if consumed {
            events.push(AgentEvent::DeliveryAcknowledged {
                message_id: identity.message_id.clone(),
            });
        }
        events.push(AgentEvent::Done {
            status: DoneStatus::Interrupted, result: None,
            error: consumed.then(|| "This message was already accepted before restart. Review the conversation and send a new message to continue.".into()),
            session_id: Some(session),
        });
        return Ok(Box::pin(futures::stream::iter(events).map(move |event| {
            let _held = (&store, &lease);
            Ok(event)
        })));
    }
    let pending = identity.as_ref().is_some_and(|identity| {
        saved
            .as_ref()
            .is_some_and(|state| state.notifications.contains(&identity.message_id))
    });
    if pending && let Some(saved) = &saved {
        request.model = Some(saved.model.clone());
        request.reasoning = saved.reasoning.as_ref().and_then(|value| {
            serde_json::from_value(serde_json::Value::String(value.clone())).ok()
        });
    }
    drop(saved);
    let options = harness.options(&request, &stop).await?;
    let selected_model = options.model.clone();
    let initial_images = if pending {
        vec![]
    } else {
        images(&request.attachments).await?
    };
    let tools = host::tools(&request, &harness.device_id)?;
    let callbacks = host::callbacks(&mut controls);
    let mode = Arc::new(Mutex::new(host::mode(&request)));
    let (event_tx, mut events) = mpsc::channel(256);
    let notices = Arc::new(Notices(OnceLock::new()));
    let native = Arc::new(OnceLock::<Arc<NativeTools>>::new());
    let app = Arc::new(Application {
        models: harness.models.clone(),
        events: event_tx.clone(),
        stop: stop.clone(),
        overflow: std::sync::atomic::AtomicBool::new(false),
        callbacks: callbacks.clone(),
        mode: mode.clone(),
        tools,
        cwd: cwd.clone(),
        uploads: harness.uploads.clone(),
    });
    let host_app = app.clone();
    let native_slot = native.clone();
    let notice_host = notices.clone();
    let profile = harness.profile.clone();
    let command_host = harness.command_host.clone();
    let root_id = root.clone();
    let mode_source = mode.clone();
    let engine = Engine::open_native_owned(store, options, move |directory| async move {
        let mut processes = ProcessOptions::new(root_id, directory.join("artifacts"), notice_host)?;
        processes.command_host = command_host;
        let mut env_command = tokio::process::Command::new("node");
        let env_command = tokio::task::spawn_blocking(move || {
            zuse_providers::compose_login_shell_path(&mut env_command);
            env_command
        })
        .await
        .map_err(|_| Error::Storage)?;
        let mut environment = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
        for (key, value) in env_command.as_std().get_envs() {
            if let Some(value) = value {
                environment.insert(
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                );
            }
        }
        processes.env = Some(environment);
        let native = Arc::new(
            NativeTools::open(NativeOptions {
                cwd: host_app.cwd.clone(),
                lock_directory: profile.join("harness/file-locks"),
                processes,
                runtime_mode: Arc::new(move || *host::lock(&mode_source)),
                callbacks: Some(callbacks),
                ripgrep: std::env::var_os("ZUSE_RG_EXECUTABLE")
                    .unwrap_or_else(|| "rg".into())
                    .into(),
            })
            .await?,
        );
        let composed = native.clone().attach(host_app as Arc<dyn Host>)?;
        let _ = native_slot.set(native);
        Ok(composed)
    })
    .await
    .map_err(failure)?;
    let _ = notices.0.set(engine.downgrade());
    engine.set_permission_mode(host::permission_mode(&request));
    let native = native
        .get()
        .cloned()
        .ok_or_else(|| failure("Native tools did not initialize"))?;
    let (tx, rx) = mpsc::channel(256);
    let stream_stop = stop.clone();
    tokio::spawn(async move {
        let _lease = controls.execution_lease.take();
        let mut mapper = Mapper::new(root.clone(), request.cwd.clone(), session.clone());
        let started = AgentEvent::SessionStarted {
            harness: HarnessId::Zuse,
            model: selected_model,
            tools: engine.tool_names(),
            cwd: request.cwd.clone(),
            session_id: session.clone(),
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
        };
        let mut task = None;
        let mut working = true;
        let mut admitted = false;
        let mut stopped = false;
        let mut admission = tokio_util::sync::CancellationToken::new();
        let initial_delivery = (!resuming_request)
            .then(|| {
                identity
                    .as_ref()
                    .map(|identity| identity.message_id.clone())
            })
            .flatten();
        if let Some(id) = &initial_delivery {
            controls.turn.register_steer(id);
        }
        if controls.turn.stop_requested().now_or_never().is_some() {
            stopped = true;
            admission.cancel();
            if let Some(id) = &initial_delivery {
                if let Err(error) = engine.cancel_deliveries(std::slice::from_ref(id)).await {
                    let _ = emit(&tx, Err(failure(error)), &stop).await;
                    stop.cancel();
                }
            }
        }
        if tx.send(Ok(started)).await.is_ok() {
            task = Some(send(
                engine.clone(),
                if resuming_request {
                    String::new()
                } else {
                    request.prompt.clone()
                },
                initial_images,
                event_tx.clone(),
                admission.clone(),
                initial_delivery.clone(),
            ));
        } else {
            stop.cancel();
        }
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
        let mut steering_open = true;
        loop {
            controls.turn.set_background(
                engine
                    .background_count()
                    .saturating_sub(usize::from(working))
                    + native
                        .processes
                        .list(&root)
                        .iter()
                        .filter(|p| p.status == ProcessStatus::Running)
                        .count(),
            );
            tokio::select! {
                biased;
                _=stop.cancelled()=>break,
                _=tx.closed()=>break,
                _=controls.turn.stop_requested()=>{
                    admission.cancel();
                    stopped = true;
                    if engine.stop_turn().await.is_err() { break; }
                    let pending = mapper.steers.iter().chain(initial_delivery.iter()).filter(|id| controls.turn.steer_cancelled(id)).cloned().collect::<Vec<_>>();
                    match engine.cancel_deliveries(&pending).await {
                        Ok(cancelled) => for id in cancelled { mapper.steers.remove(&id); controls.turn.forget_steer(&id); },
                        Err(error) => { let _ = emit(&tx, Err(failure(error)), &stop).await; break; }
                    }
                },
                event=events.recv()=>{
                    let Some(event) = event else { break; };
                    let is_root = Mapper::agent(&event) == root;
                    if is_root { admitted = true; }
                    let ended = is_root && matches!(event, Event::Completed { .. } | Event::Paused { .. });
                    if ended {
                        if let Some(task) = task.take() { let _ = task.await; }
                        working = false;
                    }
                    let route = match engine.route(Mapper::agent(&event)).await {
                        Ok(route) => route,
                        Err(error) => { let _ = emit(&tx, Err(failure(error)), &stop).await; break; }
                    };
                    if let Event::MailConsumed { delivery_id, .. } = &event { controls.turn.forget_steer(delivery_id); }
                    let translated = mapper.translate(event, &route, stopped);
                    if ended { controls.turn.set_background(engine.background_count() + native.processes.list(&root).iter().filter(|p| p.status == ProcessStatus::Running).count()); }
                    for event in translated { if !emit(&tx, Ok(event), &stop).await { stop.cancel(); break; } }
                    if ended { stopped = false; }
                },
                steer=controls.steering.recv(), if steering_open && (!working || admitted)=>{
                    let Some(steer) = steer else { steering_open = false; continue; };
                    let result = async {
                        let id = steer.message_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                        controls.turn.register_steer(&id);
                        if controls.turn.steer_cancelled(&id) || engine.delivery_cancelled(&id).await {
                            engine.cancel_deliveries(std::slice::from_ref(&id)).await.map_err(failure)?;
                            controls.turn.forget_steer(&id);
                            return Ok(());
                        }
                        if engine.acknowledge_consumed_delivery(&id).await.map_err(failure)? {
                            mapper.replays.insert(id);
                            return Ok(());
                        }
                        if let Some(config) = steer.config {
                            if tokio::fs::canonicalize(&config.cwd).await? != PathBuf::from(&request.cwd) || (config.mcp.is_some() && config.mcp != request.mcp) { return Err(failure("The next prompt needs a different native runtime")); }
                            engine.reconfigure(harness.options(&config, &stop).await?).await.map_err(failure)?;
                            *host::lock(&mode) = host::mode(&config);
                            engine.set_permission_mode(host::permission_mode(&config));
                        }
                        let attachments = images(&steer.attachments).await?;
                        mapper.steers.insert(id.clone());
                        engine.enqueue(id, &root, steer.prompt, attachments, working).await.map_err(failure)?;
                        if !working { admission = tokio_util::sync::CancellationToken::new(); admitted = false; stopped = false; working = true; task = Some(send(engine.clone(), String::new(), vec![], event_tx.clone(), admission.clone(), None)); }
                        Ok::<_, HarnessError>(())
                    }.await;
                    if let Err(error) = result { let _ = emit(&tx, Err(error), &stop).await; break; }
                },
                _=ticker.tick()=>{},
            }
        }
        stop.cancel();
        admission.cancel();
        // Freeze runners before withdrawing only the deliveries explicitly
        // cancelled by the host. A dropped stream must retain retryable mail.
        let _ = engine.interrupt(None).await;
        let cancelled = mapper
            .steers
            .iter()
            .chain(initial_delivery.iter())
            .filter(|id| controls.turn.steer_cancelled(id))
            .cloned()
            .collect::<Vec<_>>();
        if let Err(error) = engine.cancel_deliveries(&cancelled).await {
            let _ = tx.try_send(Err(failure(error)));
        }
        let _ = engine.close().await;
        if let Some(task) = task {
            let _ = task.await;
        }
        controls.turn.set_background(0);
        harness.models.remove_root(&root);
        let overflow = app.overflow.load(std::sync::atomic::Ordering::Acquire);
        if !tx.is_closed() && (working || overflow) {
            let _ = tx.try_send(Ok(AgentEvent::Done { status: if overflow { DoneStatus::Errored } else { DoneStatus::Interrupted }, result: None,
                error: overflow.then(|| "The native event buffer filled. The durable conversation is preserved; continue after inspecting its last step.".into()), session_id: Some(session) }));
        }
    });
    let guard = stream_stop.drop_guard();
    Ok(Box::pin(futures::stream::unfold(
        (rx, guard),
        |(mut rx, guard)| async move { rx.recv().await.map(|event| (event, (rx, guard))) },
    )))
}
