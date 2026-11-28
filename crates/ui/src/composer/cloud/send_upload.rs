//! A message's files on their way with it, in a cloud chat that already
//! exists. The message reaches the server once its files are available:
//!
//! ```text
//! send ─▶ computer connected ─▶ files straight to it ─▶ message sent with their paths
//!            │                     └─ fails, or no progress for STALL ─┐
//!            ├─ asleep: wake it, wait up to WAKE_WAIT ─────────────────┤
//!            └─ awake but not connected for CONNECT_WAIT ──────────────┤
//!                                                                      ▼
//!                     server storage (deleted once the computer has them)
//!                       └─ message sent with the stored files
//!                       └─ fails ─▶ the outbox retries the whole send under the
//!                                   same ids, then shows it as not sent
//! ```
//!
//! The tray shows the upload's progress meanwhile. Files already stored stay
//! stored across retries, and every attempt carries the message's original
//! ids, so it is never sent or stored twice.
use super::launch_upload;
use super::*;
use std::time::Duration;

/// Files going to a connected computer that make no progress for this long
/// go to server storage instead.
pub(super) const STALL: Duration = Duration::from_secs(30);
/// How long an awake computer's connection may take to come up before its
/// files go to server storage.
pub(super) const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// How long a sleeping computer may take to wake before its files go to
/// server storage (the server wakes it for the message anyway).
pub(super) const WAKE_WAIT: Duration = Duration::from_secs(45);

/// The files of the message being sent, while they upload.
#[derive(Clone)]
pub(in crate::composer) struct Upload {
    /// The message they belong to.
    message: String,
    /// What the tray shows: the current attempt's bytes.
    progress: launch_upload::Progress,
    /// Files already in server storage: a retry doesn't store them again.
    stored: Option<launch_upload::Progress>,
}

/// Why a message's files aren't available yet.
pub(super) enum Failed {
    /// The composer went away.
    Gone,
    /// Neither the computer nor server storage took them.
    Unavailable(String),
}

/// Whether a message already carries its files: paths on its computer, or
/// files in server storage.
pub(super) fn delivered(request: &Value, files: usize) -> bool {
    let queue = request["action"]["type"] == "queue";
    let (paths, stored) = if queue {
        (
            &request["action"]["input"]["computerFiles"],
            &request["action"]["input"]["attachments"],
        )
    } else {
        (
            &request["action"]["payload"]["request"]["attachments"],
            &request["action"]["attachments"],
        )
    };
    let count = |value: &Value| value.as_array().map_or(0, Vec::len);
    files > 0 && (count(paths) == files || count(stored) == files)
}

/// Whether a message's files are in server storage.
pub(super) fn stored_on_server(request: &Value) -> bool {
    let stored = if request["action"]["type"] == "queue" {
        &request["action"]["input"]["attachments"]
    } else {
        &request["action"]["attachments"]
    };
    stored.as_array().is_some_and(|stored| !stored.is_empty())
}

/// Record the files stored on the server for a message.
pub(super) fn set_stored_files(request: &mut Value, assets: Vec<Value>) {
    if request["action"]["type"] == "queue" {
        request["action"]["input"]["attachments"] = json!(assets);
    } else {
        request["action"]["attachments"] = json!(assets);
    }
}

impl Composer {
    /// The tray's line while the message's files upload.
    pub(super) fn cloud_send_upload(&self) -> Option<&launch_upload::Progress> {
        self.cloud.task.as_ref()?;
        let upload = self.cloud.send_upload.as_ref()?;
        let pending = self.cloud.pending_native.as_ref()?;
        (pending["action"]["messageId"] == upload.message.as_str()).then_some(&upload.progress)
    }
    /// The progress for a new attempt of `message`'s files; to server storage
    /// it resumes the files an earlier attempt already stored.
    fn start_send_upload(
        &mut self,
        message: &str,
        staged: &[StagedAttachment],
        server: bool,
    ) -> launch_upload::Progress {
        let previous = self
            .cloud
            .send_upload
            .take()
            .filter(|upload| upload.message == message)
            .and_then(|upload| upload.stored);
        let progress = if server {
            launch_upload::Progress::resume(staged, previous.as_ref())
        } else {
            launch_upload::Progress::new(staged)
        };
        self.cloud.send_upload = Some(Upload {
            message: message.to_owned(),
            progress: progress.clone(),
            stored: if server {
                Some(progress.clone())
            } else {
                previous
            },
        });
        progress
    }
    /// The files reached their destination: nothing left to show.
    pub(super) fn finish_send_upload(&mut self) {
        self.cloud.send_upload = None;
    }
}

/// Get `request`'s files where its computer can read them: straight to the
/// computer while it is connected, else (or if that fails or stalls) to
/// server storage. On success `request` carries them.
pub(super) async fn deliver(
    this: &gpui::WeakEntity<Composer>,
    cx: &mut gpui::AsyncApp,
    target: &crate::state::CloudTarget,
    request: &mut Value,
    staged: &[StagedAttachment],
) -> Result<(), Failed> {
    let message = request["action"]["messageId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let stored_before = this
        .read_with(cx, |this, _| {
            this.cloud
                .send_upload
                .as_ref()
                .is_some_and(|upload| upload.message == message && upload.stored.is_some())
        })
        .map_err(|_| Failed::Gone)?;
    // A retry after the files began going to server storage finishes there.
    if !stored_before {
        let sleeping = this
            .read_with(cx, |this, _| {
                this.cloud.native.as_ref().is_some_and(|task| task.wake)
            })
            .map_err(|_| Failed::Gone)?;
        let wait = if sleeping { WAKE_WAIT } else { CONNECT_WAIT };
        if let Some(engine) = connected_computer(this, cx, wait).await? {
            let progress = this
                .update(cx, |this, cx| {
                    let progress = this.start_send_upload(&message, staged, false);
                    cx.notify();
                    progress
                })
                .map_err(|_| Failed::Gone)?;
            match to_computer(this, cx, &engine, staged, &progress).await? {
                Ok(paths) => {
                    set_computer_files(request, paths);
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!(%error, "a message's files go to server storage instead of its computer");
                }
            }
        }
    }
    let progress = this
        .update(cx, |this, cx| {
            let progress = this.start_send_upload(&message, staged, true);
            cx.notify();
            progress
        })
        .map_err(|_| Failed::Gone)?;
    let executor = cx.background_executor().clone();
    let call = launch_upload::asset_call(
        &target.controller,
        &target.workspace,
        &target.subject,
        target.organization.as_deref(),
    );
    let upload = launch_upload::upload_files(&call, &executor, staged, &progress);
    if let Err(failure) = painting(this, cx, upload).await? {
        return Err(Failed::Unavailable(format!(
            "{} couldn't be uploaded: {}",
            failure.name, failure.error
        )));
    }
    set_stored_files(request, progress.assets(staged));
    Ok(())
}

/// The computer's connection, once it is up within `wait`. A sleeping
/// computer is asked to wake. `None` when it isn't up in time.
async fn connected_computer(
    this: &gpui::WeakEntity<Composer>,
    cx: &mut gpui::AsyncApp,
    wait: Duration,
) -> Result<Option<crate::state::EngineHandle>, Failed> {
    let ready = |cx: &mut gpui::AsyncApp| {
        this.read_with(cx, |this, cx| {
            let state = this.state.read(cx);
            (state.connection == crate::state::ConnectionStatus::Ready)
                .then(|| state.engine().cloned())
                .flatten()
        })
        .map_err(|_| Failed::Gone)
    };
    if let Some(engine) = ready(cx)? {
        return Ok(Some(engine));
    }
    let _ = this.update(cx, |_, cx| cx.emit(ComposerEvent::CloudWakeRequested));
    let executor = cx.background_executor().clone();
    let started = executor.now();
    while executor.now().duration_since(started) < wait {
        executor.timer(Duration::from_millis(250)).await;
        if let Some(engine) = ready(cx)? {
            return Ok(Some(engine));
        }
    }
    Ok(None)
}

/// Upload straight to the connected computer, two files at a time. Stops
/// with an error when it fails or makes no progress for [`STALL`].
async fn to_computer(
    this: &gpui::WeakEntity<Composer>,
    cx: &mut gpui::AsyncApp,
    engine: &crate::state::EngineHandle,
    staged: &[StagedAttachment],
    progress: &launch_upload::Progress,
) -> Result<Result<Vec<String>, String>, Failed> {
    use futures::{StreamExt as _, TryStreamExt as _};
    let executor = cx.background_executor().clone();
    let done = std::cell::Cell::new(0usize);
    let sent = progress.counter();
    let upload = futures::stream::iter(staged.iter())
        .map(|attachment| {
            let done = &done;
            let executor = &executor;
            let sent = sent.clone();
            async move {
                let path = attachments::upload_attachment(
                    engine,
                    executor,
                    Some(attachments::COMPUTER),
                    &attachment.id,
                    attachment,
                    Some(sent),
                )
                .await;
                done.set(done.get() + 1);
                path
            }
        })
        .buffered(2)
        .try_collect::<Vec<_>>();
    futures::pin_mut!(upload);
    let mut seen = (progress.sent(), done.get());
    let mut moved = executor.now();
    loop {
        let paint = executor.timer(launch_upload::PAINT_INTERVAL);
        match futures::future::select(upload.as_mut(), paint).await {
            futures::future::Either::Left((result, _)) => {
                return Ok(result.map_err(|error| error.to_string()));
            }
            futures::future::Either::Right(_) => {
                this.update(cx, |_, cx| cx.notify())
                    .map_err(|_| Failed::Gone)?;
                let now = (progress.sent(), done.get());
                if now != seen {
                    seen = now;
                    moved = executor.now();
                } else if executor.now().duration_since(moved) >= STALL {
                    return Ok(Err(format!(
                        "no progress for {}s on the computer's connection",
                        STALL.as_secs()
                    )));
                }
            }
        }
    }
}

/// Run `work`, repainting the tray's progress while it does.
async fn painting<T>(
    this: &gpui::WeakEntity<Composer>,
    cx: &mut gpui::AsyncApp,
    work: impl std::future::Future<Output = T>,
) -> Result<T, Failed> {
    let executor = cx.background_executor().clone();
    futures::pin_mut!(work);
    loop {
        let paint = executor.timer(launch_upload::PAINT_INTERVAL);
        match futures::future::select(work.as_mut(), paint).await {
            futures::future::Either::Left((result, _)) => return Ok(result),
            futures::future::Either::Right(_) => {
                this.update(cx, |_, cx| cx.notify())
                    .map_err(|_| Failed::Gone)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use sha2::Digest as _;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn digest(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(bytes))
    }

    /// How the computer's connection answers uploads.
    #[derive(Clone, Copy, PartialEq)]
    enum Link {
        /// Every request is answered.
        Healthy,
        /// Requests are taken and never answered (a half-open connection).
        Dead,
        /// Each chunk waits for a permit from the test.
        Gated,
    }

    type Received = Mutex<HashMap<String, Vec<(u64, Vec<u8>)>>>;

    /// The cloud server (commands and server storage) and the computer's
    /// connection, recording what each received.
    struct World {
        link: Mutex<Link>,
        permits: tokio::sync::Semaphore,
        server_online: AtomicBool,
        /// Bytes received per upload id, on the computer and in storage.
        computer: Received,
        storage: Received,
        /// Upload ids committed to storage, each time.
        stored: Mutex<Vec<String>>,
        /// Every command attempt, and the admitted requests in order.
        attempts: Mutex<Vec<String>>,
        admitted: Mutex<Vec<Value>>,
    }
    impl World {
        fn new(link: Link) -> Arc<Self> {
            Arc::new(Self {
                link: Mutex::new(link),
                permits: tokio::sync::Semaphore::new(0),
                server_online: AtomicBool::new(true),
                computer: Default::default(),
                storage: Default::default(),
                stored: Default::default(),
                attempts: Default::default(),
                admitted: Default::default(),
            })
        }
        fn assemble(chunks: &[(u64, Vec<u8>)]) -> Vec<u8> {
            let mut chunks = chunks.to_vec();
            chunks.sort_by_key(|(seq, _)| *seq);
            chunks.dedup_by_key(|(seq, _)| *seq);
            chunks.into_iter().flat_map(|(_, bytes)| bytes).collect()
        }
        fn admitted_ids(&self) -> Vec<String> {
            self.admitted
                .lock()
                .unwrap()
                .iter()
                .map(|request| request["commandId"].as_str().unwrap().to_owned())
                .collect()
        }
    }
    fn chunk(params: &Value) -> (String, u64, Vec<u8>) {
        (
            params["uploadId"].as_str().unwrap().to_owned(),
            params["seq"].as_u64().unwrap(),
            base64::engine::general_purpose::STANDARD
                .decode(params["data"].as_str().unwrap())
                .unwrap(),
        )
    }

    struct Server(Arc<World>);
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Server {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            let world = &self.0;
            if method == "ClaimComputerPrimary" {
                return Ok(zuse_rpc::RpcReply::Value(json!({"deviceId":null})));
            }
            if !world.server_online.load(Ordering::SeqCst) {
                return Err(zuse_rpc::RpcError::Capability(
                    zuse_protocol::cloud_task_commands::TASK_UNREACHABLE.into(),
                ));
            }
            if method == methods::CLOUD_TASK_ASSET {
                let inner = &params["params"];
                if params["method"] == methods::UPLOAD_CHUNK {
                    let (id, seq, bytes) = chunk(inner);
                    world
                        .storage
                        .lock()
                        .unwrap()
                        .entry(id)
                        .or_default()
                        .push((seq, bytes));
                    return Ok(zuse_rpc::RpcReply::Value(json!({"data":{}})));
                }
                let id = inner["uploadId"].as_str().unwrap().to_owned();
                let bytes = World::assemble(&world.storage.lock().unwrap()[&id]);
                world.stored.lock().unwrap().push(id.clone());
                return Ok(zuse_rpc::RpcReply::Value(json!({"data":{"asset":{
                    "assetId":id,"sizeBytes":bytes.len(),"sha256":digest(&bytes)}}})));
            }
            assert_eq!(method, methods::CLOUD_TASK_COMMAND);
            let request = params["request"].clone();
            let id = request["commandId"].as_str().unwrap().to_owned();
            world.attempts.lock().unwrap().push(id.clone());
            let mut admitted = world.admitted.lock().unwrap();
            if !admitted.iter().any(|seen| seen["commandId"] == id.as_str()) {
                admitted.push(request);
            }
            Ok(zuse_rpc::RpcReply::Value(
                json!({"accountId":"owner","organizationId":null,
                    "data":{"commandId":id,"durablyAdmitted":true}}),
            ))
        }
    }

    struct Computer(Arc<World>);
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Computer {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            let world = &self.0;
            assert_eq!(params["computer"], true, "{method} goes to the computer");
            let link = *world.link.lock().unwrap();
            if link == Link::Dead {
                futures::future::pending::<()>().await;
            }
            if method == methods::UPLOAD_CHUNK {
                if link == Link::Gated {
                    world.permits.acquire().await.unwrap().forget();
                }
                let (id, seq, bytes) = chunk(&params);
                world
                    .computer
                    .lock()
                    .unwrap()
                    .entry(id)
                    .or_default()
                    .push((seq, bytes));
                return Ok(zuse_rpc::RpcReply::Value(json!({})));
            }
            let id = params["uploadId"].as_str().unwrap().to_owned();
            Ok(zuse_rpc::RpcReply::Value(
                json!({"path":format!("/home/zuse/uploads/{id}")}),
            ))
        }
    }

    struct Fixture {
        runtime: tokio::runtime::Runtime,
        world: Arc<World>,
        state: Entity<crate::state::AppState>,
        composer: Entity<Composer>,
    }
    impl Fixture {
        /// An existing, awake cloud chat whose computer is connected.
        fn new(link: Link, cx: &mut gpui::TestAppContext) -> Self {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let world = World::new(link);
            let (controller, engine) = {
                let _entered = runtime.enter();
                (
                    crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
                        Arc::new(Server(world.clone())),
                    )),
                    crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
                        Arc::new(Computer(world.clone())),
                    )),
                )
            };
            let state = cx.new(|_| {
                let mut state = crate::state::AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                state.selected_chat = Some("session".into());
                state.connection = crate::state::ConnectionStatus::Ready;
                state.set_test_engine(engine);
                state
            });
            let composer = cx.new(|cx| Composer::new(state.clone(), cx));
            composer.update(cx, |c, cx| {
                let target = crate::state::CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                };
                c.set_cloud_task(
                    target,
                    json!({"agent":"zuse","model":"test-model","runtimeMode":"full-access","queueRunSelectionVersion":1}),
                    cx,
                );
            });
            Self {
                runtime,
                world,
                state,
                composer,
            }
        }
        fn settle(&self, cx: &mut gpui::TestAppContext) {
            let _entered = self.runtime.enter();
            for _ in 0..12 {
                cx.run_until_parked();
                self.runtime.block_on(async {
                    tokio::task::yield_now().await;
                });
            }
            cx.run_until_parked();
        }
        /// Let `time` pass, a paint at a time.
        fn wait(&self, time: Duration, cx: &mut gpui::TestAppContext) {
            let steps = time.as_millis() / launch_upload::PAINT_INTERVAL.as_millis();
            for _ in 0..steps {
                cx.executor().advance_clock(launch_upload::PAINT_INTERVAL);
                self.settle(cx);
            }
        }
        fn send(&self, text: &str, files: &[StagedAttachment], cx: &mut gpui::TestAppContext) {
            let _entered = self.runtime.enter();
            self.composer.update(cx, |c, cx| {
                c.input.update(cx, |input, cx| input.set_text(text, cx));
                c.attachments.insert(c.current_key.clone(), files.to_vec());
                assert!(c.send_cloud_task(text.into(), false, cx));
                assert!(c.staged().is_empty(), "its files leave the draft");
            });
        }
        fn upload_label(&self, cx: &mut gpui::TestAppContext) -> Option<String> {
            self.composer.read_with(cx, |c, _| {
                c.cloud_send_upload().map(|upload| upload.label())
            })
        }
        /// Every file arrived intact, at the computer or in storage.
        fn assert_intact(&self, files: &[StagedAttachment], storage: bool) {
            let received = if storage {
                self.world.storage.lock().unwrap()
            } else {
                self.world.computer.lock().unwrap()
            };
            for file in files {
                let bytes = World::assemble(&received[&file.id]);
                assert_eq!(digest(&bytes), digest(file.bytes()), "{} intact", file.name);
            }
        }
    }

    /// Three distinct images, 300 KB each.
    fn images() -> Vec<StagedAttachment> {
        (0..3u8)
            .map(|n| {
                StagedAttachment::from_file(
                    &format!("shot-{n}.png"),
                    (0..300_000u32).map(|i| (i % 251) as u8 ^ n).collect(),
                )
            })
            .collect()
    }

    #[gpui::test]
    fn files_go_straight_to_a_connected_computer(cx: &mut gpui::TestAppContext) {
        let f = Fixture::new(Link::Healthy, cx);
        let files = images();
        f.send("look at these", &files, cx);
        f.wait(Duration::from_secs(1), cx);
        let admitted = f.world.admitted.lock().unwrap().clone();
        assert_eq!(admitted.len(), 1);
        let run = &admitted[0]["action"];
        assert_eq!(
            run["payload"]["request"]["attachments"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(
            run.get("attachments").is_none(),
            "nothing in server storage"
        );
        assert!(f.world.stored.lock().unwrap().is_empty());
        f.assert_intact(&files, false);
        assert_eq!(
            f.upload_label(cx),
            None,
            "the upload line goes once they land"
        );
    }

    #[gpui::test]
    fn a_dead_computer_connection_falls_back_to_server_storage_and_delivers(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = Fixture::new(Link::Dead, cx);
        let files = images();
        f.send("look at these", &files, cx);
        f.settle(cx);
        // The message waits for its files, showing the upload, and isn't
        // with the server yet.
        assert_eq!(
            f.upload_label(cx).as_deref(),
            Some("Uploading 3 files · 0%")
        );
        assert!(f.world.attempts.lock().unwrap().is_empty());
        // No progress for STALL: the files go to server storage instead.
        f.wait(STALL + Duration::from_secs(1), cx);
        let admitted = f.world.admitted.lock().unwrap().clone();
        assert_eq!(admitted.len(), 1, "delivered once");
        let stored = admitted[0]["action"]["attachments"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(stored.len(), 3);
        for (asset, file) in stored.iter().zip(&files) {
            assert_eq!(asset["assetId"], file.id.as_str(), "in their order");
            assert_eq!(asset["sha256"], digest(file.bytes()));
        }
        assert_eq!(
            admitted[0]["action"]["payload"]["request"]["attachments"],
            json!([]),
            "no computer paths"
        );
        f.assert_intact(&files, true);
        assert_eq!(f.world.stored.lock().unwrap().len(), 3, "each stored once");
        f.composer.read_with(cx, |c, _| {
            assert!(c.cloud.pending_native.is_none());
            assert!(c.cloud_send_upload().is_none());
            assert!(c.failure.is_none());
        });
    }

    #[gpui::test]
    fn a_slow_upload_shows_its_progress_and_never_falls_back_while_it_moves(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = Fixture::new(Link::Gated, cx);
        let chunk = crate::attachments::UPLOAD_CHUNK_B64_CHARS / 4 * 3;
        let file = StagedAttachment::from_file("big.png", vec![9u8; chunk * 3]);
        f.send("big one", std::slice::from_ref(&file), cx);
        f.settle(cx);
        assert_eq!(
            f.upload_label(cx).as_deref(),
            Some("Uploading big.png · 0%")
        );
        // A chunk lands every 20 s: slow, but never stalled.
        for expected in ["33%", "66%"] {
            f.wait(Duration::from_secs(20), cx);
            f.world.permits.add_permits(1);
            f.wait(Duration::from_secs(1), cx);
            assert_eq!(
                f.upload_label(cx),
                Some(format!("Uploading big.png · {expected}"))
            );
        }
        f.world.permits.add_permits(1);
        f.wait(Duration::from_secs(1), cx);
        let admitted = f.world.admitted.lock().unwrap().clone();
        assert_eq!(admitted.len(), 1);
        assert_eq!(
            admitted[0]["action"]["payload"]["request"]["attachments"],
            json!([format!("/home/zuse/uploads/{}", file.id)])
        );
        assert!(
            f.world.stored.lock().unwrap().is_empty(),
            "never in storage"
        );
        f.assert_intact(std::slice::from_ref(&file), false);
    }

    #[gpui::test]
    fn an_unconnected_computer_times_out_to_server_storage(cx: &mut gpui::TestAppContext) {
        let f = Fixture::new(Link::Healthy, cx);
        f.state.update(cx, |state, _| {
            state.connection = crate::state::ConnectionStatus::Connecting;
        });
        let files = images();
        f.send("look", &files, cx);
        f.wait(CONNECT_WAIT - Duration::from_secs(1), cx);
        assert!(
            f.world.storage.lock().unwrap().is_empty(),
            "waits briefly first"
        );
        f.wait(Duration::from_secs(2), cx);
        let admitted = f.world.admitted.lock().unwrap().clone();
        assert_eq!(admitted.len(), 1);
        assert_eq!(
            admitted[0]["action"]["attachments"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(f.world.computer.lock().unwrap().is_empty());
        f.assert_intact(&files, true);
    }

    #[gpui::test]
    fn a_long_failure_shows_not_sent_and_retry_neither_stores_nor_sends_twice(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = Fixture::new(Link::Dead, cx);
        f.world.server_online.store(false, Ordering::SeqCst);
        let files = images();
        f.send("look at these", &files, cx);
        f.wait(outbox::OFFLINE_LIMIT + Duration::from_secs(60), cx);
        f.composer.read_with(cx, |c, _| {
            assert!(c.cloud_messages_not_sent(), "shown as not sent");
            assert!(c.cloud_activity().is_none());
            assert!(c.cloud_send_upload().is_none());
        });
        assert!(f.world.admitted.lock().unwrap().is_empty());
        // Retry once the server answers: stored and sent once.
        f.world.server_online.store(true, Ordering::SeqCst);
        {
            let _entered = f.runtime.enter();
            f.composer
                .update(cx, |c, cx| c.retry_cloud_messages_not_sent(cx));
        }
        f.wait(Duration::from_secs(5), cx);
        // A second Retry has nothing left to send.
        {
            let _entered = f.runtime.enter();
            f.composer
                .update(cx, |c, cx| c.retry_cloud_messages_not_sent(cx));
        }
        f.wait(Duration::from_secs(5), cx);
        let admitted = f.world.admitted.lock().unwrap().clone();
        assert_eq!(admitted.len(), 1);
        assert_eq!(
            admitted[0]["action"]["attachments"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let stored = f.world.stored.lock().unwrap().clone();
        assert_eq!(stored.len(), 3, "each file committed once: {stored:?}");
        f.assert_intact(&files, true);
        f.composer.read_with(cx, |c, _| {
            assert!(!c.cloud_messages_not_sent());
            assert!(c.cloud.pending_native.is_none());
        });
    }

    #[gpui::test]
    fn remove_takes_a_not_sent_message_back_to_the_draft_with_its_files(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = Fixture::new(Link::Dead, cx);
        f.world.server_online.store(false, Ordering::SeqCst);
        let files = images();
        f.send("look at these", &files, cx);
        f.wait(outbox::OFFLINE_LIMIT + Duration::from_secs(60), cx);
        assert!(f.composer.read_with(cx, |c, _| c.cloud_messages_not_sent()));
        {
            let _entered = f.runtime.enter();
            f.composer
                .update(cx, |c, cx| c.remove_cloud_message_not_sent(cx));
        }
        f.composer.read_with(cx, |c, cx| {
            assert!(!c.cloud_messages_not_sent());
            assert!(c.cloud.pending_native.is_none());
            assert_eq!(c.input.read(cx).text(), "look at these");
            assert_eq!(c.staged().len(), 3);
        });
        f.state
            .read_with(cx, |state, _| assert!(state.pending_echoes().is_empty()));
    }

    #[gpui::test]
    fn a_message_sent_behind_a_stalled_one_goes_after_it_in_order_once(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = Fixture::new(Link::Dead, cx);
        let files = images();
        f.send("look at these", &files, cx);
        f.settle(cx);
        // A text message while the first one's files are stuck.
        f.send("hello?", &[], cx);
        f.settle(cx);
        assert!(f.world.attempts.lock().unwrap().is_empty());
        f.wait(STALL + Duration::from_secs(2), cx);
        let admitted = f.world.admitted.lock().unwrap().clone();
        let prompts: Vec<_> = admitted
            .iter()
            .map(|request| request["action"]["payload"]["request"]["prompt"].clone())
            .collect();
        assert_eq!(prompts, [json!("look at these"), json!("hello?")]);
        let attempts = f.world.attempts.lock().unwrap().clone();
        assert_eq!(attempts, f.world.admitted_ids(), "each sent once");
        f.composer.read_with(cx, |c, _| {
            assert!(c.cloud.pending_native.is_none());
            assert!(c.cloud.outbox.later.is_empty());
        });
    }
}
