//! The connector against an in-memory cloud: approvals, streaming, stop,
//! deadlines, outages, restarts and account changes.
use super::*;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Cloud {
    /// Approved requests waiting for this host to claim them.
    approved: VecDeque<Value>,
    /// Requests the owner has stopped or whose grant ended.
    revoked: Vec<uuid::Uuid>,
    /// Every output a running request published, in order.
    progress: HashMap<uuid::Uuid, Vec<String>>,
    finished: HashMap<uuid::Uuid, (String, String)>,
    offers: Vec<Instant>,
    /// Requests are dropped as if the network were down.
    offline: bool,
    /// Finish reports are refused with this status (a rejected proof, say).
    refuse_finish: Option<u16>,
    /// Authorization checks never answer.
    hang_checks: bool,
}

#[derive(Default, Clone)]
struct Fake(Arc<Mutex<Cloud>>);
impl Fake {
    fn approve(&self, command: &str, timeout_ms: u64) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        self.0.lock().unwrap().approved.push_back(
            json!({"id":id,"command":command,"cwd":std::env::temp_dir(),"timeoutMs":timeout_ms}),
        );
        id
    }
    fn set_offline(&self, offline: bool) {
        self.0.lock().unwrap().offline = offline;
    }
    fn revoke(&self, id: uuid::Uuid) {
        self.0.lock().unwrap().revoked.push(id);
    }
    fn finished(&self, id: uuid::Uuid) -> Option<(String, String)> {
        self.0.lock().unwrap().finished.get(&id).cloned()
    }
    async fn wait_finished(&self, id: uuid::Uuid, within: Duration) -> (String, String) {
        let deadline = Instant::now() + within;
        loop {
            if let Some(done) = self.finished(id) {
                return done;
            }
            assert!(Instant::now() < deadline, "request {id} never finished");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
#[async_trait::async_trait]
impl Transport for Fake {
    async fn send(&self, body: Value) -> Result<Value, SendError> {
        if body["action"] == "check" && self.0.lock().unwrap().hang_checks {
            std::future::pending::<()>().await;
        }
        let mut cloud = self.0.lock().unwrap();
        if cloud.offline {
            return Err(SendError::Unavailable("offline".into()));
        }
        if body["action"] == "finish" {
            if let Some(status) = cloud.refuse_finish {
                return Err(SendError::Rejected(status));
            }
        }
        let request = || uuid::Uuid::parse_str(body["request"].as_str().unwrap()).unwrap();
        Ok(match body["action"].as_str().unwrap() {
            "offer" => {
                cloud.offers.push(Instant::now());
                Value::Null
            }
            "claim" => cloud.approved.pop_front().unwrap_or(Value::Null),
            "check" => {
                let id = request();
                if cloud.revoked.contains(&id) || cloud.finished.contains_key(&id) {
                    json!(false)
                } else {
                    cloud
                        .progress
                        .entry(id)
                        .or_default()
                        .push(body["output"].as_str().unwrap_or_default().to_owned());
                    json!(true)
                }
            }
            "finish" => {
                let id = request();
                if cloud.finished.contains_key(&id) {
                    return Err(SendError::Rejected(409));
                }
                cloud.finished.insert(
                    id,
                    (
                        body["status"].as_str().unwrap().to_owned(),
                        body["output"].as_str().unwrap().to_owned(),
                    ),
                );
                Value::Null
            }
            other => panic!("unexpected action {other}"),
        })
    }
}

fn timing() -> Timing {
    Timing {
        heartbeat: Duration::from_millis(50),
        offline_limit: Duration::from_millis(600),
        backoff_min: Duration::from_millis(40),
        backoff_max: Duration::from_millis(320),
        stable_after: Duration::from_secs(60),
    }
}

struct Running {
    fake: Fake,
    journal: tempfile::TempDir,
    stop: CancellationToken,
    status: watch::Receiver<BridgeStatus>,
    handle: tokio::task::JoinHandle<Result<Exit>>,
}

fn start_in(fake: &Fake, journal: tempfile::TempDir, owner: Arc<AtomicBool>) -> Running {
    let (status, receiver) = watch::channel(BridgeStatus::default());
    let host = Host::new(
        vec!["/bin/sh".into()],
        true,
        std::env::temp_dir(),
        "me".into(),
        journal.path().to_owned(),
        Arc::new(fake.clone()),
        timing(),
        Arc::new(move || owner.load(Ordering::SeqCst)),
        status,
    )
    .unwrap();
    let stop = CancellationToken::new();
    let handle = tokio::spawn(Arc::new(host).run(stop.clone()));
    Running {
        fake: fake.clone(),
        journal,
        stop,
        status: receiver,
        handle,
    }
}

fn start(fake: &Fake) -> Running {
    start_in(
        fake,
        tempfile::tempdir().unwrap(),
        Arc::new(AtomicBool::new(true)),
    )
}

/// Running, as opposed to exited (a killed child may briefly linger unreaped).
fn alive(pid: i32) -> bool {
    let exists = unsafe { libc::kill(pid, 0) == 0 };
    let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .map(|(_, rest)| rest.starts_with('Z'))
        })
        .unwrap_or(false);
    exists && !zombie
}

async fn gone(pid: i32) -> bool {
    for _ in 0..100 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

async fn read_pid(path: &Path) -> i32 {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("background process never started");
}

#[tokio::test]
async fn approved_command_streams_output_before_exit_and_reports_its_status() {
    let fake = Fake::default();
    let id = fake.approve(
        "printf 'Unicode ✓ $(literal)'; sleep 0.4; printf ' done' >&2; exit 7",
        10_000,
    );
    let host = start(&fake);
    let (status, output) = fake.wait_finished(id, Duration::from_secs(5)).await;
    assert_eq!(status, "exit status: 7");
    assert_eq!(output, "Unicode ✓ $(literal) done");
    let seen = fake.0.lock().unwrap().progress[&id].clone();
    assert!(
        seen.iter().any(|output| output == "Unicode ✓ $(literal)"),
        "output was visible while it ran: {seen:?}"
    );
    assert_eq!(host.status.borrow().state, "online");
    // The server confirmed it, so the receipt is gone.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(std::fs::read_dir(host.journal.path()).unwrap().count(), 0);
    host.stop.cancel();
    assert_eq!(host.handle.await.unwrap().unwrap(), Exit::Shutdown);
}

#[tokio::test]
async fn stop_kills_the_whole_process_group_and_keeps_partial_output() {
    let fake = Fake::default();
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let id = fake.approve(
        &format!(
            "sleep 300 & echo $! > '{}'; printf started; wait",
            pid_file.display()
        ),
        600_000,
    );
    let _host = start(&fake);
    let background = read_pid(&pid_file).await;
    assert!(alive(background));
    tokio::time::sleep(Duration::from_millis(120)).await;
    fake.revoke(id);
    let (status, output) = fake.wait_finished(id, Duration::from_secs(3)).await;
    assert_eq!(status, "stopped: approval ended");
    assert_eq!(output, "started");
    assert!(gone(background).await, "background child survived Stop");
}

#[tokio::test]
async fn each_command_uses_its_own_deadline() {
    let fake = Fake::default();
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let short = fake.approve(
        &format!(
            "sleep 300 & echo $! > '{}'; printf begun; wait",
            pid_file.display()
        ),
        1_000,
    );
    let long = fake.approve("sleep 1.5; printf finished", 30_000);
    let _host = start(&fake);
    let started = Instant::now();
    let descendant = read_pid(&pid_file).await;
    let (status, output) = fake.wait_finished(short, Duration::from_secs(4)).await;
    assert_eq!(status, "timed out after 1s");
    assert_eq!(output, "begun");
    assert!(gone(descendant).await, "the deadline left a child running");
    assert!(started.elapsed() < Duration::from_secs(3));
    // The second ran alongside it and outlived the first one's deadline.
    let (status, output) = fake.wait_finished(long, Duration::from_secs(4)).await;
    assert_eq!(
        (status.as_str(), output.as_str()),
        ("exit status: 0", "finished")
    );
}

#[tokio::test]
async fn short_outages_are_ridden_out_and_long_ones_stop_the_command() {
    let fake = Fake::default();
    let survives = fake.approve("sleep 0.8; printf ok", 30_000);
    let host = start(&fake);
    tokio::time::sleep(Duration::from_millis(150)).await;
    fake.set_offline(true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(host.status.borrow().state, "reconnecting");
    fake.set_offline(false);
    let (status, output) = fake.wait_finished(survives, Duration::from_secs(4)).await;
    assert_eq!((status.as_str(), output.as_str()), ("exit status: 0", "ok"));
    assert_eq!(host.status.borrow().state, "online");

    let stopped = fake.approve("sleep 30", 60_000);
    while !fake.0.lock().unwrap().progress.contains_key(&stopped) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    fake.set_offline(true);
    // Fails closed: unreachable past the limit means the grant can't be
    // confirmed. Its result is delivered once the cloud is back.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(fake.finished(stopped).is_none());
    fake.set_offline(false);
    let (status, _) = fake.wait_finished(stopped, Duration::from_secs(3)).await;
    assert_eq!(status, "stopped: connection lost");
}

#[tokio::test]
async fn an_admitted_request_is_never_started_twice_and_finished_ones_republish() {
    let fake = Fake::default();
    let journal = tempfile::tempdir().unwrap();
    let marker = journal
        .path()
        .join("..")
        .join(format!("ran-{}", uuid::Uuid::new_v4()));
    // A run admitted before a crash, without a result: its outcome is unknown.
    let admitted = uuid::Uuid::new_v4();
    std::fs::write(
        journal.path().join(admitted.to_string()),
        r#"{"state":"admitted"}"#,
    )
    .unwrap();
    // A run that finished but whose result never reached the cloud.
    let finished = uuid::Uuid::new_v4();
    std::fs::write(
        journal.path().join(finished.to_string()),
        json!({"task":uuid::Uuid::new_v4(),"state":"finished","status":"exit status: 0","output":"saved"})
            .to_string(),
    )
    .unwrap();
    fake.0.lock().unwrap().approved.push_back(json!({"id":admitted,
        "command":format!("touch '{}'", marker.display()),"cwd":std::env::temp_dir(),"timeoutMs":5000}));
    let host = start_in(&fake, journal, Arc::new(AtomicBool::new(true)));
    let (status, output) = fake.wait_finished(finished, Duration::from_secs(2)).await;
    assert_eq!(
        (status.as_str(), output.as_str()),
        ("exit status: 0", "saved")
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!marker.exists(), "an admitted request ran again");
    assert!(fake.finished(admitted).is_none());
    assert!(host.journal.path().join(admitted.to_string()).exists());
    assert!(!host.journal.path().join(finished.to_string()).exists());
}

#[tokio::test]
async fn shutdown_stops_running_commands_and_records_why() {
    let fake = Fake::default();
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let id = fake.approve(
        &format!("echo $$ > '{}'; exec sleep 300", pid_file.display()),
        600_000,
    );
    let host = start(&fake);
    let pid = read_pid(&pid_file).await;
    host.stop.cancel();
    assert_eq!(host.handle.await.unwrap().unwrap(), Exit::Shutdown);
    assert_eq!(
        fake.finished(id).map(|done| done.0).as_deref(),
        Some("stopped: connector shut down")
    );
    assert!(gone(pid).await);
}

#[tokio::test]
async fn signing_out_stops_commands_and_pauses_until_the_owner_returns() {
    let fake = Fake::default();
    let owner = Arc::new(AtomicBool::new(true));
    let id = fake.approve("sleep 30", 600_000);
    let (status, mut updates) = watch::channel(BridgeStatus::default());
    let stop = CancellationToken::new();
    let journal = tempfile::tempdir().unwrap();
    let factory = {
        let fake = fake.clone();
        let owner = owner.clone();
        let journal = journal.path().to_owned();
        move |status| {
            let owner = owner.clone();
            Host::new(
                vec!["/bin/sh".into()],
                true,
                std::env::temp_dir(),
                "me".into(),
                journal.clone(),
                Arc::new(fake.clone()),
                timing(),
                Arc::new(move || owner.load(Ordering::SeqCst)),
                status,
            )
        }
    };
    let watched = owner.clone();
    let supervisor = tokio::spawn(supervise(
        factory,
        Arc::new(move || watched.load(Ordering::SeqCst)),
        None,
        timing(),
        status,
        stop.clone(),
    ));
    while !fake.0.lock().unwrap().progress.contains_key(&id) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    owner.store(false, Ordering::SeqCst);
    let (done, _) = fake.wait_finished(id, Duration::from_secs(2)).await;
    assert_eq!(done, "stopped: signed out");
    updates
        .wait_for(|s| s.state == "paused")
        .await
        .expect("paused while signed out");
    let offers = fake.0.lock().unwrap().offers.len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fake.0.lock().unwrap().offers.len(),
        offers,
        "offered while paused"
    );
    owner.store(true, Ordering::SeqCst);
    // Without an account watch the supervisor rechecks at its backoff ceiling.
    tokio::time::timeout(
        Duration::from_secs(2),
        updates.wait_for(|s| s.state == "online"),
    )
    .await
    .expect("resumed for the owner")
    .unwrap();
    stop.cancel();
    supervisor.await.unwrap();
    assert_eq!(updates.borrow().state, "off");
}

#[tokio::test]
async fn outages_reconnect_with_growing_backoff_and_show_why() {
    let fake = Fake::default();
    fake.set_offline(true);
    let (status, mut updates) = watch::channel(BridgeStatus::default());
    let stop = CancellationToken::new();
    let journal = tempfile::tempdir().unwrap();
    let attempts = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let factory = {
        let fake = fake.clone();
        let journal = journal.path().to_owned();
        let attempts = attempts.clone();
        move |status| {
            attempts.lock().unwrap().push(Instant::now());
            Host::new(
                vec!["/bin/sh".into()],
                true,
                std::env::temp_dir(),
                "me".into(),
                journal.clone(),
                Arc::new(fake.clone()),
                timing(),
                Arc::new(|| true),
                status,
            )
        }
    };
    let supervisor = tokio::spawn(supervise(
        factory,
        Arc::new(|| true),
        None,
        timing(),
        status,
        stop.clone(),
    ));
    updates
        .wait_for(|s| s.state == "reconnecting")
        .await
        .unwrap();
    assert!(
        updates
            .borrow()
            .error
            .as_deref()
            .unwrap()
            .contains("offline")
    );
    tokio::time::sleep(Duration::from_millis(900)).await;
    let gaps: Vec<Duration> = attempts
        .lock()
        .unwrap()
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    assert!(gaps.len() >= 3, "{gaps:?}");
    assert!(gaps[2] > gaps[0], "backoff did not grow: {gaps:?}");
    assert!(
        gaps.iter().all(|gap| *gap < Duration::from_millis(500)),
        "backoff exceeded its ceiling: {gaps:?}"
    );
    fake.set_offline(false);
    tokio::time::timeout(
        Duration::from_secs(2),
        updates.wait_for(|s| s.state == "online" && s.error.is_none()),
    )
    .await
    .unwrap()
    .unwrap();
    stop.cancel();
    supervisor.await.unwrap();
}

#[test]
fn long_output_keeps_its_start_and_end() {
    let mut capture = Capture::default();
    capture.push(&vec![b'a'; HEAD_BYTES]);
    for _ in 0..10 {
        capture.push(&vec![b'b'; 1000]);
    }
    capture.push(&vec![b'c'; TAIL_BYTES]);
    let text = capture.render();
    assert!(text.starts_with(&"a".repeat(HEAD_BYTES)));
    assert!(text.ends_with(&"c".repeat(TAIL_BYTES)));
    assert!(text.contains("[… 10000 bytes omitted …]"));
    assert_eq!(
        text.len(),
        HEAD_BYTES + TAIL_BYTES + "\n[… 10000 bytes omitted …]\n".len()
    );
}

#[test]
fn engine_connector_resumes_only_for_the_profile_that_enabled_it() {
    let profile = tempfile::tempdir().unwrap();
    assert!(read_enabled(profile.path()).is_none());
    let enabled = Enabled {
        owner: "user_1".into(),
        device_id: "mac".into(),
        label: "Mac".into(),
    };
    write_enabled(profile.path(), &enabled).unwrap();
    assert_eq!(read_enabled(profile.path()), Some(enabled));
    std::fs::write(enabled_path(profile.path()), "{").unwrap();
    assert!(read_enabled(profile.path()).is_none());
}

/// Sending from this computer makes it the chat's primary only with its own
/// enrolled key and only for the account that enabled it; from anywhere else
/// the chat's primary is cleared.
#[tokio::test]
async fn sending_claims_primary_with_this_computers_key_or_clears_it() {
    use axum::{
        Json, Router,
        extract::{Path as UrlPath, State},
        http::{HeaderMap, StatusCode},
        routing::{delete, post},
    };
    type Seen = Arc<Mutex<Vec<(String, Value)>>>;
    async fn claim(
        State(seen): State<Seen>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let proof = headers["dpop"].to_str().unwrap();
        use base64::Engine as _;
        let claims: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(proof.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        seen.lock().unwrap().push((
            "claim".into(),
            json!({"body":body,
            "htm":claims["htm"],"path":claims["htu"].as_str().unwrap().split_once("/v1").unwrap().1,
            "bound":claims["ath"].is_string()}),
        ));
        Json(json!({"deviceId":"this-mac"}))
    }
    async fn clear(
        State(seen): State<Seen>,
        UrlPath((workspace, chat)): UrlPath<(String, String)>,
        axum::extract::Query(query): axum::extract::Query<
            std::collections::HashMap<String, String>,
        >,
    ) -> StatusCode {
        seen.lock().unwrap().push((
            "clear".into(),
            json!([workspace, chat, query.get("commandId")]),
        ));
        StatusCode::NO_CONTENT
    }
    let seen = Seen::default();
    let router = Router::new()
        .route("/v1/cloud/bridge/primary", post(claim))
        .route(
            "/v1/cloud/workspaces/{workspace}/chats/{chat}/primary-computer",
            delete(clear),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let profile = tempfile::tempdir().unwrap();
    let account =
        crate::account::Account::fixture_signed_in(profile.path(), origin.clone(), "user_a");

    // Not enabled here: the user isn't typing on an enabled computer.
    let none = claim_primary_at(
        &origin,
        profile.path(),
        &account,
        "user_a",
        ("w1", "s_1", Some("cmd-1")),
    )
    .await
    .unwrap();
    assert_eq!(none, None);
    // Enabled by another account: never claimed for this one.
    let enabled = |owner: &str| Enabled {
        owner: owner.into(),
        device_id: "this-mac".into(),
        label: "Mac".into(),
    };
    write_enabled(profile.path(), &enabled("user_b")).unwrap();
    claim_primary_at(
        &origin,
        profile.path(),
        &account,
        "user_a",
        ("w1", "s_1", Some("cmd-1")),
    )
    .await
    .unwrap();
    write_enabled(profile.path(), &enabled("user_a")).unwrap();
    let claimed = claim_primary_at(
        &origin,
        profile.path(),
        &account,
        "user_a",
        ("w1", "s_1", Some("cmd-1")),
    )
    .await
    .unwrap();
    assert_eq!(claimed.as_deref(), Some("this-mac"));
    // A different signed-in account than the chat's owner is refused outright.
    assert!(
        claim_primary_at(
            &origin,
            profile.path(),
            &account,
            "user_b",
            ("w1", "s_1", None)
        )
        .await
        .is_err()
    );
    assert!(
        claim_primary_at(
            &origin,
            profile.path(),
            &account,
            "user_a",
            ("../w", "s_1", None)
        )
        .await
        .is_err()
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("clear".to_owned(), json!(["w1", "s_1", "cmd-1"])),
            ("clear".to_owned(), json!(["w1", "s_1", "cmd-1"])),
            (
                "claim".to_owned(),
                json!({"body":{"workspaceId":"w1","chatId":"s_1","commandId":"cmd-1"},"htm":"POST",
                    "path":"/cloud/bridge/primary","bound":true})
            ),
        ]
    );
    server.abort();
}

#[test]
fn the_cloud_address_must_be_https_or_local() {
    assert_eq!(
        origin("https://api.zuse.sh").unwrap(),
        "https://api.zuse.sh"
    );
    assert_eq!(
        origin("http://127.0.0.1:28700/").unwrap(),
        "http://127.0.0.1:28700"
    );
    for bad in [
        "http://api.zuse.sh",
        "https://user:pw@api.zuse.sh",
        "https://api.zuse.sh/v1",
        "ftp://api.zuse.sh",
    ] {
        assert!(origin(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn a_result_the_cloud_refuses_to_authenticate_is_kept_for_later() {
    let fake = Fake::default();
    fake.0.lock().unwrap().refuse_finish = Some(401);
    let id = fake.approve("printf done", 10_000);
    let host = start(&fake);
    let receipt = host.journal.path().join(id.to_string());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(fake.finished(id).is_none());
    let saved: Value = serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(saved["state"], "finished");
    // Accepted once the cloud takes it again; only then is the receipt gone.
    fake.0.lock().unwrap().refuse_finish = None;
    let (status, _) = fake.wait_finished(id, Duration::from_secs(3)).await;
    assert_eq!(status, "exit status: 0");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!receipt.exists());
}

#[tokio::test]
async fn a_check_that_never_answers_stops_the_command_at_the_offline_limit() {
    let fake = Fake::default();
    let id = fake.approve("sleep 30", 60_000);
    let _host = start(&fake);
    while !fake.0.lock().unwrap().progress.contains_key(&id) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    fake.0.lock().unwrap().hang_checks = true;
    let (status, _) = fake.wait_finished(id, Duration::from_secs(3)).await;
    assert_eq!(status, "stopped: connection lost");
}

#[tokio::test]
async fn work_claimed_as_the_connector_stops_never_starts() {
    let fake = Fake::default();
    let journal = tempfile::tempdir().unwrap();
    let marker = journal
        .path()
        .join("..")
        .join(format!("ran-{}", uuid::Uuid::new_v4()));
    let (status, _) = watch::channel(BridgeStatus::default());
    let host = Arc::new(
        Host::new(
            vec!["/bin/sh".into()],
            true,
            std::env::temp_dir(),
            "me".into(),
            journal.path().to_owned(),
            Arc::new(fake.clone()),
            timing(),
            Arc::new(|| true),
            status,
        )
        .unwrap(),
    );
    let id = uuid::Uuid::new_v4();
    let stopped = CancellationToken::new();
    stopped.cancel();
    host.clone()
        .run_one(
            Claimed {
                id,
                command: format!("touch '{}'", marker.display()),
                cwd: std::env::temp_dir(),
                timeout_ms: Some(5000),
            },
            stopped,
        )
        .await;
    assert!(!marker.exists());
    assert_eq!(
        fake.finished(id).map(|done| done.0).as_deref(),
        Some("stopped before start")
    );
}

#[tokio::test]
async fn signing_out_while_offline_keeps_the_result_and_pauses() {
    let fake = Fake::default();
    let owner = Arc::new(AtomicBool::new(true));
    let id = fake.approve("sleep 30", 600_000);
    let host = start_in(&fake, tempfile::tempdir().unwrap(), owner.clone());
    while !fake.0.lock().unwrap().progress.contains_key(&id) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    fake.set_offline(true);
    owner.store(false, Ordering::SeqCst);
    let exit = tokio::time::timeout(Duration::from_secs(3), host.handle)
        .await
        .expect("the host let go after sign-out")
        .unwrap()
        .unwrap();
    assert_eq!(exit, Exit::AccountChanged);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(host.journal.path().join(id.to_string())).unwrap())
            .unwrap();
    assert_eq!(saved["status"], "stopped: signed out");
}

#[tokio::test]
async fn stop_reports_when_the_saved_choice_cannot_be_forgotten() {
    let profile = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(enabled_path(profile.path())).unwrap();
    let connector = Connector::default();
    assert!(connector.stop(profile.path()).await.is_err());
    assert_eq!(connector.status().state, "off");
    std::fs::remove_dir(enabled_path(profile.path())).unwrap();
    assert!(connector.stop(profile.path()).await.is_ok());
}
