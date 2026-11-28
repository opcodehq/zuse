//! A new chat's first-message files upload while its computer starts:
//!
//! ```text
//! Send ─▶ create (files declared, bytes not sent yet) ─▶ accepted, computer starting
//!           └─▶ files upload to the new computer's storage ─▶ all committed ─▶ chat opens
//!                 ├─ a slow or dropped request times out and is sent again, with backoff
//!                 └─ out of attempts ─▶ Retry (same files, same computer) / Remove
//! ```
//!
//! The server holds the first turn until every declared file is committed,
//! so the turn starts with its files. That hold ends after
//! `PENDING_ASSET_WAIT_MS` (15 minutes): past it the computer takes the turn
//! and reports the missing file. The whole upload therefore gets
//! [`UPLOAD_BUDGET`], well inside that hold, and stops with Retry and Remove
//! when it runs out, before the turn could start without its file.
use super::*;
use base64::Engine as _;
use futures::FutureExt as _;
use sha2::Digest as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use zuse_protocol::cloud_assets::{CHUNK_BYTES, committed_name, mime_for_name};

/// One chunk's request; a slow link still moves 512 KB well within this.
pub(super) const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);
/// The commit reads every chunk back on the server.
pub(super) const COMMIT_TIMEOUT: Duration = Duration::from_secs(150);
/// Tries per request before the upload shows Retry and Remove.
pub(super) const ATTEMPTS: u32 = 5;
/// Chunks in flight at once; chunks are idempotent by sequence.
const CONCURRENCY: usize = 2;
/// The whole upload's deadline, inside the server's first-turn hold.
pub(super) const UPLOAD_BUDGET: Duration = Duration::from_secs(12 * 60);
/// How often the progress label repaints while bytes move.
pub(super) const PAINT_INTERVAL: Duration = Duration::from_millis(250);

/// The wait before try `attempt + 1`: 1 s, 2 s, 4 s, 8 s, at most 16 s.
pub(super) fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.saturating_sub(1).min(4)).min(16))
}

/// One upload call (`UploadChunk` or `UploadCommit`) for the new computer.
pub(super) type Call<'a> = attachments::UploadCall<'a>;

/// Progress of a launch's uploads, shared with the status line.
#[derive(Clone, Default)]
pub(in crate::composer) struct Progress {
    sent: Arc<AtomicU64>,
    /// Requests waiting to be sent again.
    retrying: Arc<AtomicUsize>,
    /// Files already committed, with what the server recorded for each: a
    /// Retry skips them.
    committed: Arc<std::sync::Mutex<std::collections::HashMap<String, Value>>>,
    total: u64,
    name: String,
    files: usize,
    /// The file that ran out of attempts: Remove takes it out of the draft.
    pub(super) failed: Option<String>,
}

impl Progress {
    pub(super) fn new(files: &[StagedAttachment]) -> Self {
        Self {
            total: files.iter().map(|file| file.bytes().len() as u64).sum(),
            name: files
                .first()
                .map(|file| file.name.clone())
                .unwrap_or_default(),
            files: files.len(),
            ..Default::default()
        }
    }
    /// Progress for a Retry: files `previous` committed count as sent and
    /// are not uploaded again.
    pub(super) fn resume(files: &[StagedAttachment], previous: Option<&Progress>) -> Self {
        let progress = Self::new(files);
        if let Some(previous) = previous {
            let committed = previous.committed.lock().unwrap().clone();
            let sent: u64 = files
                .iter()
                .filter(|file| committed.contains_key(&file.id))
                .map(|file| file.bytes().len() as u64)
                .sum();
            progress.sent.store(sent, Ordering::Relaxed);
            *progress.committed.lock().unwrap() = committed;
        }
        progress
    }
    /// The sent-bytes counter, for an upload that counts its own bytes.
    pub(super) fn counter(&self) -> Arc<AtomicU64> {
        self.sent.clone()
    }
    pub(super) fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed).min(self.total)
    }
    pub(super) fn percent(&self) -> u64 {
        (self.sent() * 100).checked_div(self.total).unwrap_or(0)
    }
    /// "Uploading shot.png · 42%", or "Retrying upload · 42%" while a slow or
    /// dropped request is being sent again.
    pub(super) fn label(&self) -> String {
        let percent = self.percent();
        if self.retrying.load(Ordering::Relaxed) > 0 {
            return format!("Retrying upload · {percent}%");
        }
        if self.files == 1 {
            format!("Uploading {} · {percent}%", self.name)
        } else {
            format!("Uploading {} files · {percent}%", self.files)
        }
    }
    /// What the server recorded for each of `files` that has committed, in
    /// their order.
    pub(super) fn assets(&self, files: &[StagedAttachment]) -> Vec<Value> {
        let committed = self.committed.lock().unwrap();
        files
            .iter()
            .filter_map(|file| committed.get(&file.id).cloned())
            .collect()
    }
    /// The status line's detail: bytes sent of the total.
    pub(super) fn detail(&self) -> String {
        format!("{} of {}", megabytes(self.sent()), megabytes(self.total))
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// The create request's `pendingAttachments`: what each file's commit will
/// record. `None` when a file's type can't be stored, so the launch takes
/// the older path, which explains why.
pub(super) fn pending_attachments(files: &[StagedAttachment]) -> Option<Vec<Value>> {
    files
        .iter()
        .map(|file| {
            let name = committed_name(&file.name);
            let mime = mime_for_name(name)?;
            Some(json!({
                "uploadId": file.id,
                "mimeType": mime,
                "originalName": name,
                "sizeBytes": file.bytes().len(),
                "sha256": digest(file.bytes()),
            }))
        })
        .collect()
}

fn digest(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(bytes))
}

/// Why the uploads stopped: the file and the last error.
#[derive(Debug)]
pub(super) struct Failure {
    pub file: String,
    pub name: String,
    pub error: String,
}

/// Upload every file to a computer's server storage in `CHUNK_BYTES`
/// pieces, the size the server's commit expects.
/// Each request is raced against its timeout and sent again with backoff;
/// `progress` counts committed chunk bytes.
pub(super) async fn upload_files(
    call: &Call<'_>,
    executor: &gpui::BackgroundExecutor,
    files: &[StagedAttachment],
    progress: &Progress,
) -> Result<(), Failure> {
    let current = std::cell::RefCell::new(None::<&StagedAttachment>);
    let upload = async {
        for file in files {
            if progress.committed.lock().unwrap().contains_key(&file.id) {
                continue;
            }
            *current.borrow_mut() = Some(file);
            let asset = upload_file(call, executor, file, progress)
                .await
                .map_err(|error| Failure {
                    file: file.id.clone(),
                    name: file.name.clone(),
                    error,
                })?;
            progress
                .committed
                .lock()
                .unwrap()
                .insert(file.id.clone(), asset);
        }
        Ok(())
    };
    futures::pin_mut!(upload);
    match futures::future::select(upload, executor.timer(UPLOAD_BUDGET)).await {
        futures::future::Either::Left((result, _)) => result,
        futures::future::Either::Right(_) => {
            let file = current.borrow().or(files.first());
            Err(Failure {
                file: file.map(|file| file.id.clone()).unwrap_or_default(),
                name: file.map(|file| file.name.clone()).unwrap_or_default(),
                error: format!("upload exceeded {}s", UPLOAD_BUDGET.as_secs()),
            })
        }
    }
}

/// Marks one request as waiting to be sent again until it settles (or is
/// dropped), so the label says "Retrying" while any request is.
struct Retrying<'a>(&'a AtomicUsize);
impl<'a> Retrying<'a> {
    fn start(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self(count)
    }
}
impl Drop for Retrying<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn upload_file(
    call: &Call<'_>,
    executor: &gpui::BackgroundExecutor,
    file: &StagedAttachment,
    progress: &Progress,
) -> Result<Value, String> {
    use futures::TryStreamExt as _;
    let bytes = file.bytes();
    let name = committed_name(&file.name);
    let chunks: Vec<(usize, &[u8])> = bytes.chunks(CHUNK_BYTES).enumerate().collect();
    futures::stream::iter(chunks.into_iter().map(Ok::<_, String>))
        .try_for_each_concurrent(CONCURRENCY, |(seq, chunk)| async move {
            let params = json!({
                "uploadId": file.id,
                "seq": seq,
                "data": base64::engine::general_purpose::STANDARD.encode(chunk),
            });
            send(
                call,
                executor,
                methods::UPLOAD_CHUNK,
                params,
                CHUNK_TIMEOUT,
                progress,
            )
            .await?;
            progress
                .sent
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
            Ok(())
        })
        .await?;
    let reply = send(
        call,
        executor,
        methods::UPLOAD_COMMIT,
        json!({"uploadId": file.id, "fileName": name}),
        COMMIT_TIMEOUT,
        progress,
    )
    .await?;
    // The committed file must be exactly the attached one.
    let asset = &reply["asset"];
    if asset["sizeBytes"] != bytes.len() || asset["sha256"] != digest(bytes) {
        return Err("the stored file doesn't match the attached one".into());
    }
    Ok(asset.clone())
}

/// One request with its timeout, sent again with backoff until [`ATTEMPTS`].
async fn send(
    call: &Call<'_>,
    executor: &gpui::BackgroundExecutor,
    method: &'static str,
    params: Value,
    timeout: Duration,
    progress: &Progress,
) -> Result<Value, String> {
    let mut attempt = 1;
    let mut retrying = None;
    loop {
        let request = call(method, params.clone(), timeout);
        let timer = executor.timer(timeout);
        let result = match futures::future::select(request, timer).await {
            futures::future::Either::Left((result, _)) => result,
            futures::future::Either::Right(_) => Err(format!("{method} timed out")),
        };
        match result {
            Ok(value) => return Ok(value),
            Err(error) if attempt < ATTEMPTS => {
                tracing::warn!(%error, attempt, method, "cloud launch upload retry");
                retrying.get_or_insert_with(|| Retrying::start(&progress.retrying));
                executor.timer(backoff(attempt)).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// [`Call`] routed to `workspace`'s server storage through the controller.
pub(super) fn asset_call<'a>(
    controller: &'a crate::state::EngineHandle,
    workspace: &'a str,
    account: &'a str,
    organization: Option<&'a str>,
) -> impl Fn(&'static str, Value, Duration) -> futures::future::LocalBoxFuture<'a, Result<Value, String>>
+ 'a {
    move |method, params, _timeout| {
        async move {
            controller
                .client()
                .call_scoped(
                    methods::CLOUD_TASK_ASSET,
                    json!({"workspaceId":workspace,"expectedAccountId":account,
                        "expectedOrganizationId":organization,"method":method,"params":params}),
                )
                .await
                .map(|reply| reply["data"].clone())
                .map_err(|error| error.to_string())
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn file(name: &str, len: usize) -> StagedAttachment {
        StagedAttachment::from_file(name, vec![7u8; len])
    }

    /// Replies to chunk and commit calls like the server would.
    fn reply(method: &str, params: &Value, files: &[StagedAttachment]) -> Value {
        if method == methods::UPLOAD_COMMIT {
            let file = files
                .iter()
                .find(|file| file.id == params["uploadId"])
                .unwrap();
            return json!({"asset":{"sizeBytes":file.bytes().len(),"sha256":digest(file.bytes())}});
        }
        json!({})
    }

    #[test]
    fn pending_attachments_declare_what_the_commit_records() {
        let shot = file("shot.png", 10);
        let pending = pending_attachments(std::slice::from_ref(&shot)).unwrap();
        assert_eq!(pending[0]["uploadId"], shot.id.as_str());
        assert_eq!(pending[0]["mimeType"], "image/png");
        assert_eq!(pending[0]["originalName"], "shot.png");
        assert_eq!(pending[0]["sizeBytes"], 10);
        assert_eq!(pending[0]["sha256"], digest(shot.bytes()));
        assert!(pending_attachments(&[file("tool.exe", 3)]).is_none());
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let waits: Vec<u64> = (1..=7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 16, 16]);
    }

    /// A slow upload moves the label's percentage chunk by chunk.
    #[gpui::test]
    async fn a_slow_upload_shows_its_progress(cx: &mut gpui::TestAppContext) {
        let files = vec![file("shot.png", CHUNK_BYTES * 2 + 10)];
        let progress = Progress::new(&files);
        assert_eq!(progress.label(), "Uploading shot.png · 0%");
        let executor = cx.background_executor.clone();
        let seen = files.clone();
        let call = move |method: &'static str, params: Value, _: Duration| {
            let executor = executor.clone();
            let value = reply(method, &params, &seen);
            async move {
                executor.timer(Duration::from_secs(10)).await;
                Ok(value)
            }
            .boxed_local()
        };
        let executor = cx.background_executor.clone();
        let upload = upload_files(&call, &executor, &files, &progress);
        futures::pin_mut!(upload);
        assert!(futures::poll!(upload.as_mut()).is_pending());
        cx.background_executor
            .advance_clock(Duration::from_secs(10));
        assert!(futures::poll!(upload.as_mut()).is_pending());
        // Two chunks in flight finished together; the third is still moving.
        assert_eq!(progress.sent(), (CHUNK_BYTES * 2) as u64);
        assert_eq!(progress.label(), "Uploading shot.png · 99%");
        assert_eq!(progress.detail(), "1.0 MB of 1.0 MB");
        cx.background_executor
            .advance_clock(Duration::from_secs(10));
        assert!(futures::poll!(upload.as_mut()).is_pending(), "committing");
        cx.background_executor
            .advance_clock(Duration::from_secs(10));
        assert!(matches!(
            futures::poll!(upload.as_mut()),
            std::task::Poll::Ready(Ok(()))
        ));
        assert_eq!(progress.percent(), 100);
    }

    /// A request that never answers times out, says so, and is sent again.
    #[gpui::test]
    async fn a_stalled_upload_times_out_and_retries(cx: &mut gpui::TestAppContext) {
        let files = vec![file("notes.txt", 100)];
        let progress = Progress::new(&files);
        let calls = Rc::new(RefCell::new(Vec::<(&'static str, u64)>::new()));
        let seen = files.clone();
        let log = calls.clone();
        let call = move |method: &'static str, params: Value, _: Duration| {
            let first = !log.borrow().iter().any(|(m, _)| *m == method);
            log.borrow_mut()
                .push((method, params["seq"].as_u64().unwrap_or(0)));
            let value = reply(method, &params, &seen);
            async move {
                if first && method == methods::UPLOAD_CHUNK {
                    futures::future::pending::<()>().await;
                }
                Ok(value)
            }
            .boxed_local()
        };
        let executor = cx.background_executor.clone();
        let upload = upload_files(&call, &executor, &files, &progress);
        futures::pin_mut!(upload);
        assert!(futures::poll!(upload.as_mut()).is_pending());
        cx.background_executor.advance_clock(CHUNK_TIMEOUT);
        cx.run_until_parked();
        assert!(futures::poll!(upload.as_mut()).is_pending());
        assert_eq!(progress.label(), "Retrying upload · 0%");
        cx.background_executor.advance_clock(backoff(1));
        cx.run_until_parked();
        let result = futures::poll!(upload.as_mut());
        assert!(
            matches!(result, std::task::Poll::Ready(Ok(()))),
            "{result:?}"
        );
        let calls = calls.borrow();
        let chunks = calls
            .iter()
            .filter(|(method, _)| *method == methods::UPLOAD_CHUNK)
            .count();
        assert_eq!(chunks, 2, "the stalled chunk is sent once more");
        assert_eq!(progress.label(), "Uploading notes.txt · 100%");
    }

    /// Out of attempts, the upload stops with the file that failed.
    #[gpui::test]
    async fn an_upload_out_of_attempts_names_its_file(cx: &mut gpui::TestAppContext) {
        let files = vec![file("notes.txt", 100)];
        let progress = Progress::new(&files);
        let call = |_: &'static str, _: Value, _: Duration| {
            async { Err::<Value, _>("Cloud service is unavailable.".to_string()) }.boxed_local()
        };
        let executor = cx.background_executor.clone();
        let upload = upload_files(&call, &executor, &files, &progress);
        futures::pin_mut!(upload);
        for attempt in 1..ATTEMPTS {
            assert!(futures::poll!(upload.as_mut()).is_pending());
            cx.background_executor.advance_clock(backoff(attempt));
            cx.run_until_parked();
        }
        let std::task::Poll::Ready(Err(failure)) = futures::poll!(upload.as_mut()) else {
            panic!("the upload gives up");
        };
        assert_eq!(failure.file, files[0].id);
        assert_eq!(failure.name, "notes.txt");
        assert_eq!(failure.error, "Cloud service is unavailable.");
    }

    /// A Retry after one file committed and the next failed starts at the
    /// file that failed, with the committed one counted as sent.
    #[gpui::test]
    async fn a_retry_skips_files_already_committed(cx: &mut gpui::TestAppContext) {
        let files = vec![file("first.png", 100), file("second.png", 300)];
        let failing = Rc::new(std::cell::Cell::new(true));
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let (seen, log, fail) = (files.clone(), calls.clone(), failing.clone());
        let call = move |method: &'static str, params: Value, _: Duration| {
            let upload = params["uploadId"].as_str().unwrap().to_owned();
            log.borrow_mut().push(upload.clone());
            let refuse = fail.get() && upload == seen[1].id;
            let value = reply(method, &params, &seen);
            async move {
                if refuse {
                    Err("Cloud service is unavailable.".to_string())
                } else {
                    Ok(value)
                }
            }
            .boxed_local()
        };
        let executor = cx.background_executor.clone();
        let first = Progress::new(&files);
        let upload = upload_files(&call, &executor, &files, &first);
        futures::pin_mut!(upload);
        for attempt in 1..ATTEMPTS {
            assert!(futures::poll!(upload.as_mut()).is_pending());
            cx.background_executor.advance_clock(backoff(attempt));
        }
        assert!(matches!(
            futures::poll!(upload.as_mut()),
            std::task::Poll::Ready(Err(_))
        ));
        failing.set(false);
        calls.borrow_mut().clear();
        let retry = Progress::resume(&files, Some(&first));
        assert_eq!(retry.sent(), 100, "the committed file counts as sent");
        let upload = upload_files(&call, &executor, &files, &retry);
        futures::pin_mut!(upload);
        assert!(matches!(
            futures::poll!(upload.as_mut()),
            std::task::Poll::Ready(Ok(()))
        ));
        assert!(
            calls.borrow().iter().all(|id| *id == files[1].id),
            "only the failed file is sent again"
        );
        assert_eq!(retry.percent(), 100);
    }

    /// An upload that keeps crawling stops inside the server's first-turn
    /// hold, so Retry and Remove show before the turn could start without it.
    #[gpui::test]
    async fn a_crawling_upload_stops_within_its_budget(cx: &mut gpui::TestAppContext) {
        let files = vec![file("big.png", CHUNK_BYTES * 30)];
        let progress = Progress::new(&files);
        let executor = cx.background_executor.clone();
        let seen = files.clone();
        let call = move |method: &'static str, params: Value, _: Duration| {
            let executor = executor.clone();
            let value = reply(method, &params, &seen);
            async move {
                executor.timer(CHUNK_TIMEOUT - Duration::from_secs(1)).await;
                Ok(value)
            }
            .boxed_local()
        };
        let executor = cx.background_executor.clone();
        let upload = upload_files(&call, &executor, &files, &progress);
        futures::pin_mut!(upload);
        let mut elapsed = Duration::ZERO;
        let failure = loop {
            if let std::task::Poll::Ready(result) = futures::poll!(upload.as_mut()) {
                break result.expect_err("the budget ends the upload");
            }
            cx.background_executor.advance_clock(Duration::from_secs(1));
            elapsed += Duration::from_secs(1);
        };
        assert_eq!(elapsed, UPLOAD_BUDGET);
        assert!(
            UPLOAD_BUDGET.as_millis() < zuse_protocol::cloud_assets::PENDING_ASSET_WAIT_MS as u128
        );
        assert_eq!(failure.name, "big.png");
        assert!(progress.percent() < 100);
    }

    /// A controller that admits the create at once. Chunk uploads wait for
    /// `open` (or fail when `failing`).
    #[derive(Default)]
    struct Controller {
        calls: std::sync::Mutex<Vec<(String, Value)>>,
        open: tokio::sync::Notify,
        opened: std::sync::atomic::AtomicBool,
        failing: bool,
        /// The server refuses the create.
        refuse_create: bool,
    }
    impl Controller {
        fn methods(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(method, params)| match params["method"].as_str() {
                    Some(inner) => format!("{method}/{inner}"),
                    None => method.clone(),
                })
                .collect()
        }
        fn release(&self) {
            self.opened.store(true, Ordering::SeqCst);
            self.open.notify_waiters();
        }
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Controller {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            self.calls
                .lock()
                .unwrap()
                .push((method.into(), params.clone()));
            match method {
                methods::CLOUD_CREATE if self.refuse_create => Err(zuse_rpc::RpcError::Failed(
                    "Cloud service is unavailable.".into(),
                )),
                methods::CLOUD_CREATE => zuse_rpc::RpcReply::value(&json!({"workspace":{
                    "workspaceId":"ws","initialSessionId":"session",
                    "commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2}})),
                methods::CLOUD_TASK_ASSET if self.failing => Err(zuse_rpc::RpcError::Failed(
                    "Cloud service is unavailable.".into(),
                )),
                methods::CLOUD_TASK_ASSET if params["method"] == methods::UPLOAD_CHUNK => {
                    while !self.opened.load(Ordering::SeqCst) {
                        let wait = self.open.notified();
                        if self.opened.load(Ordering::SeqCst) {
                            break;
                        }
                        wait.await;
                    }
                    zuse_rpc::RpcReply::value(&json!({"data":{}}))
                }
                methods::CLOUD_TASK_ASSET => {
                    let bytes = vec![7u8; 1000];
                    zuse_rpc::RpcReply::value(&json!({"data":{"asset":{
                        "sizeBytes":bytes.len(),"sha256":digest(&bytes)}}}))
                }
                _ => zuse_rpc::RpcReply::value(&json!({})),
            }
        }
    }

    /// A composer about to start a new cloud chat with `shot.png` attached,
    /// on a server that holds the first turn for files still uploading.
    fn launching(
        cx: &mut gpui::TestAppContext,
        runtime: &tokio::runtime::Runtime,
        controller: Arc<Controller>,
    ) -> gpui::Entity<Composer> {
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let engine =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(controller));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            state.set_test_engine(engine);
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |c, cx| {
            let context = json!({"accountId":"fixture-user","ownerId":"fixture-user","organizationId":null,
                "commandAdmissionVersion":2,"launchAttachmentHoldVersion":1,
                "computerCapabilities":{"providers":[{"id":"boxd","configured":true,"managedBaseReady":true,
                    "commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,
                    "supportsSnapshots":true,"supportsDocker":false,
                    "sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],
                    "unavailableReason":null}]},
                "providerAgentCatalogs":{"boxd":{"providers":[{"id":"codex","name":"Codex",
                    "defaultModel":"fixture-model","models":[{"id":"fixture-model","label":"Fixture"}]}]}},
                "agentConnections":[{"agent":"codex","state":"connected","connectionId":"codex-connection"}]});
            c.pickers.update(cx, |p, cx| {
                p.seed_cloud_creation_for_test(
                    context,
                    json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd",
                        "defaultBranch":"main","canCreate":true}),
                    cx,
                )
            });
            let key = c.current_key.clone();
            c.attachments
                .entry(key)
                .or_default()
                .push(file("shot.png", 1000));
            c.input
                .update(cx, |input, cx| input.set_text("Look at this", cx));
        });
        composer
    }

    /// Run the app and the controller until `done` holds (or give up).
    fn drive(
        cx: &mut gpui::TestAppContext,
        runtime: &tokio::runtime::Runtime,
        mut done: impl FnMut(&mut gpui::TestAppContext) -> bool,
    ) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            runtime.block_on(tokio::time::sleep(Duration::from_millis(2)));
            cx.executor().advance_clock(Duration::from_secs(1));
        }
        panic!("the launch did not get there");
    }

    /// The computer is created at once; the file uploads beside it with its
    /// progress showing, and the chat opens once the file has landed.
    #[gpui::test]
    fn creation_starts_before_the_attachment_finishes_uploading(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let controller = Arc::new(Controller::default());
        let composer = launching(cx, &runtime, controller.clone());
        let admitted = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|cx| {
            let admitted = admitted.clone();
            cx.subscribe(&composer, move |_, event: &ComposerEvent, _| {
                if let ComposerEvent::CloudAdmitted { launch, .. } = event {
                    admitted.borrow_mut().push(launch.clone());
                }
            })
        });
        let id = composer.update(cx, |c, cx| {
            let id = c.staged()[0].id.clone();
            assert!(c.send_cloud("Look at this".into(), cx));
            let request = c.cloud.request.as_ref().unwrap();
            assert_eq!(request["pendingAttachments"][0]["uploadId"], id.as_str());
            assert_eq!(request["pendingAttachments"][0]["originalName"], "shot.png");
            assert_eq!(request["attachmentTransfers"], json!([]));
            id
        });
        drive(cx, &runtime, |_| {
            controller
                .methods()
                .iter()
                .any(|m| m.ends_with(methods::UPLOAD_CHUNK))
        });
        let calls = controller.methods();
        assert_eq!(calls[0], methods::CLOUD_CREATE, "{calls:?}");
        assert!(
            !calls.iter().any(|m| m.ends_with(methods::UPLOAD_COMMIT)),
            "the upload is still going: {calls:?}"
        );
        let create = controller.calls.lock().unwrap()[0].1.clone();
        assert_eq!(create["firstMessage"], "Look at this");
        assert_eq!(create["pendingAttachments"][0]["uploadId"], id.as_str());
        composer.update(cx, |c, cx| {
            assert!(c.cloud.admitted(), "the computer is starting");
            assert_eq!(
                c.cloud_creation_activity(cx),
                Some(("Uploading shot.png · 0%".into(), true))
            );
        });
        controller.release();
        drive(cx, &runtime, |_| !admitted.borrow().is_empty());
        let launch = admitted.borrow()[0].clone();
        assert_eq!(launch["firstMessage"], "Look at this");
        assert_eq!(launch["pendingAttachments"][0]["uploadId"], id.as_str());
        let chunk = controller
            .calls
            .lock()
            .unwrap()
            .iter()
            .find(|(_, p)| p["method"] == methods::UPLOAD_CHUNK)
            .map(|(_, p)| p.clone())
            .unwrap();
        assert_eq!(chunk["workspaceId"], "ws");
        assert_eq!(chunk["params"]["uploadId"], id.as_str());
        composer.update(cx, |c, _| {
            assert!(c.cloud.request.is_none());
            assert!(c.cloud.upload.is_none());
        });
    }

    /// An upload that keeps failing stops with Retry and Remove. Remove
    /// archives the started computer and keeps the message, without the
    /// file, as the draft.
    #[gpui::test]
    fn a_failed_upload_offers_retry_and_remove(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let controller = Arc::new(Controller {
            failing: true,
            ..Default::default()
        });
        let composer = launching(cx, &runtime, controller.clone());
        composer.update(cx, |c, cx| assert!(c.send_cloud("Look at this".into(), cx)));
        drive(cx, &runtime, |cx| {
            composer.read_with(cx, |c, _| c.launch_upload_failed())
        });
        let chunks = controller
            .methods()
            .iter()
            .filter(|m| m.ends_with(methods::UPLOAD_CHUNK))
            .count();
        assert_eq!(chunks, ATTEMPTS as usize, "sent again before giving up");
        composer.update(cx, |c, cx| {
            assert_eq!(
                c.cloud_creation_activity(cx),
                Some((
                    "Couldn't upload shot.png. Your message is saved here.".into(),
                    false
                ))
            );
            c.remove_failed_cloud_upload(cx);
            assert!(c.cloud.request.is_none());
            assert_eq!(c.input.read(cx).text(), "Look at this");
            assert!(c.staged().iter().all(|file| file.name != "shot.png"));
        });
        drive(cx, &runtime, |_| {
            controller
                .methods()
                .iter()
                .any(|m| m == methods::CLOUD_ARCHIVE)
        });
        let archive = controller
            .calls
            .lock()
            .unwrap()
            .iter()
            .find(|(method, _)| method == methods::CLOUD_ARCHIVE)
            .map(|(_, params)| params.clone())
            .unwrap();
        assert_eq!(archive["workspaceId"], "ws");
    }

    /// A created chat claims this computer as its primary right after the
    /// create returns; a refused create never touches the primary.
    #[gpui::test]
    fn a_created_chat_claims_its_primary_and_a_refused_one_does_not(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let controller = Arc::new(Controller::default());
        let composer = launching(cx, &runtime, controller.clone());
        composer.update(cx, |c, cx| assert!(c.send_cloud("Look at this".into(), cx)));
        drive(cx, &runtime, |_| {
            controller
                .methods()
                .iter()
                .any(|m| m == "ClaimComputerPrimary")
        });
        let calls = controller.calls.lock().unwrap().clone();
        let create = calls
            .iter()
            .position(|(m, _)| m == methods::CLOUD_CREATE)
            .unwrap();
        let claim = calls
            .iter()
            .position(|(m, _)| m == "ClaimComputerPrimary")
            .unwrap();
        assert!(create < claim, "claimed only after the create");
        assert_eq!(
            calls[claim].1,
            json!({"expectedAccountId":"fixture-user","workspaceId":"ws","chatId":"session"})
        );

        let refused = Arc::new(Controller {
            refuse_create: true,
            ..Default::default()
        });
        let composer = launching(cx, &runtime, refused.clone());
        composer.update(cx, |c, cx| assert!(c.send_cloud("Look at this".into(), cx)));
        drive(cx, &runtime, |cx| {
            composer.read_with(cx, |c, _| c.cloud.task.is_none())
        });
        let methods = refused.methods();
        assert!(methods.iter().any(|m| m == methods::CLOUD_CREATE));
        assert!(
            !methods.iter().any(|m| m == "ClaimComputerPrimary"),
            "{methods:?}"
        );
    }
}
