//! Reconciling a capture wake whose submission never got an answer.
use super::stages::{
    CAPTURE_LIMIT_MS, CAPTURE_START_GRACE_MS, EFFECT_MAX_SUBMISSIONS, EFFECT_SETTLE_MS,
    EFFECT_WAIT_LIMIT_MS,
};
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A Boxd-like source machine that was hibernated by the lifecycle pause.
struct Source {
    state: Mutex<MachineState>,
    /// What each wake submission does: `true` wakes the machine and answers,
    /// `false` fails without an answer and leaves the machine hibernated.
    wakes: Mutex<Vec<bool>>,
    wake_calls: AtomicUsize,
    captures: AtomicUsize,
    pauses: AtomicUsize,
    /// What the guest reports about a launched capture helper.
    status: Mutex<CaptureStatus>,
}
impl Source {
    fn new(state: MachineState, wakes: Vec<bool>) -> Self {
        Self {
            state: Mutex::new(state),
            wakes: Mutex::new(wakes),
            wake_calls: AtomicUsize::new(0),
            captures: AtomicUsize::new(0),
            pauses: AtomicUsize::new(0),
            status: Mutex::new(CaptureStatus::Running),
        }
    }
}
#[async_trait]
impl RecoveryProvider for Source {
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(Some(Machine {
            id: id.into(),
            name: "original".into(),
            state: self.state.lock().unwrap().clone(),
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn wake(&self, id: &str, state: &MachineState) -> Result<(), ProviderError> {
        assert_eq!(id, "machine-bob");
        assert_eq!(
            state,
            &MachineState::Paused,
            "wake is chosen for hibernation"
        );
        self.wake_calls.fetch_add(1, Ordering::SeqCst);
        let answered = self.wakes.lock().unwrap().remove(0);
        if !answered {
            return Err(ProviderError::OutcomeUnknown);
        }
        *self.state.lock().unwrap() = MachineState::Running;
        Ok(())
    }
    async fn capture(&self, plan: &CapturePlan) -> Result<(), ProviderError> {
        assert_eq!(plan.source_machine_id, "machine-bob");
        self.captures.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn capture_status(
        &self,
        id: &str,
        operation: &str,
    ) -> Result<CaptureStatus, ProviderError> {
        assert_eq!((id, operation), ("machine-bob", "archive-original"));
        Ok(*self.status.lock().unwrap())
    }
    async fn stop_capture(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        panic!("capture is not cancelled")
    }
    async fn pause(&self, id: &str) -> Result<(), ProviderError> {
        assert_eq!(id, "machine-bob");
        self.pauses.fetch_add(1, Ordering::SeqCst);
        *self.state.lock().unwrap() = MachineState::Paused;
        Ok(())
    }
    async fn retire(&self, _: &str) -> Result<(), ProviderError> {
        panic!("upload does not retire")
    }
}

struct Fixture {
    admin: PgPool,
    pool: PgPool,
    schema: String,
    control: RuntimeControl,
}
impl Fixture {
    async fn start(wake: Value) -> Self {
        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("recovery_wake_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |c, _| {
                let scope = scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scope}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for migration in [
            include_str!("../../../tests/fixtures/legacy-catalog.sql"),
            jobs::MIGRATION,
            lifecycle::MIGRATION,
            MIGRATION,
            include_str!("../../../migrations/0013_recovery_receipts.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        let signing = ed25519_dalek::SigningKey::from_bytes(&[81; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([81;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([82; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        );
        let (_, wrapped) = control
            .master
            .create_transcript_key("user-bob", "ws-bob")
            .unwrap();
        let archive = json!({"version":1,"id":"archive-original","sourceSandboxId":"machine-bob","archivedAtMs":1,"expiresAtMs":9999999999999_i64,"phase":"uploading","verifiedChunks":0});
        let config = json!({"runtimeGeneration":2,"gatewayEpoch":2,"recoveryArchive":archive,"rustRuntimeStorage":{"storageIncarnationId":"original-disk"}});
        sqlx::query("UPDATE api_cloud_workspaces SET state='archived',desired_state='archived',request_config=$1,wrapped_transcript_key=$2 WHERE workspace_id='ws-bob'").bind(config).bind(wrapped).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',2,0)")
            .execute(&pool)
            .await
            .unwrap();
        let binding = json!({"workspaceId":"ws-bob","chatId":"chat-bob","sessionId":"session-bob","rustStorage":{"accountId":"user-bob"}});
        sqlx::query("INSERT INTO zuse_rust_recovery_operations(operation_id,workspace_id,generation,source_machine_id,storage_incarnation_id,binding,phase,expires_at,created_at,updated_at,provider_effects) VALUES('archive-original','ws-bob',2,'machine-bob','original-disk',$1,'uploading',9999999999999,1,1,$2)").bind(binding).bind(json!({"wake":wake})).execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            "recover-original",
            JOB_KIND,
            "ws-bob",
            2,
            &json!({"operationId":"archive-original"}),
            clock_ms(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Self {
            admin,
            pool,
            schema,
            control,
        }
    }
    /// One worker pass over the uploading stage.
    async fn pass(&self, source: &Source) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0,lease_expires_at=0 WHERE state<>'failed'")
            .execute(&self.pool)
            .await
            .unwrap();
        let job = jobs::claim(&self.pool, JOB_KIND, "workspace", clock_ms(), LEASE)
            .await
            .unwrap()
            .expect("recovery job is claimable");
        self.control.recovery_upload(source, &job).await
    }
    async fn wake(&self) -> Value {
        sqlx::query_scalar("SELECT provider_effects->'wake' FROM zuse_rust_recovery_operations")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    /// Pretend the current wake effect was recorded `ms` ago.
    async fn age(&self, ms: i64) {
        sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,'{wake,recordedAtMs}',to_jsonb($1::bigint))").bind(clock_ms() - ms).execute(&self.pool).await.unwrap();
    }
    async fn finish(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

/// The stuck staging shape: written before effects carried a time.
fn stuck_wake() -> Value {
    json!({"state":"uncertain","receipt":null,"requestId":"original-wake"})
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn unanswered_wake_on_a_still_hibernated_machine_is_resubmitted_and_capture_proceeds() {
    let f = Fixture::start(stuck_wake()).await;
    let source = Source::new(MachineState::Paused, vec![true]);
    // The legacy effect first gets a settle window; nothing is replayed yet.
    f.pass(&source).await.unwrap();
    assert_eq!(source.wake_calls.load(Ordering::SeqCst), 0);
    let wake = f.wake().await;
    assert_eq!(wake["requestId"], "original-wake");
    assert!(wake["recordedAtMs"].is_i64());
    // Inside the window an unchanged machine proves nothing.
    f.pass(&source).await.unwrap();
    assert_eq!(source.wake_calls.load(Ordering::SeqCst), 0);
    // After it, still hibernated proves the wake never happened: submit a
    // fresh one, then capture once the machine is running.
    f.age(EFFECT_SETTLE_MS).await;
    f.pass(&source).await.unwrap();
    assert_eq!(source.wake_calls.load(Ordering::SeqCst), 1);
    let wake = f.wake().await;
    assert_eq!(wake["state"], "accepted");
    assert_eq!(wake["submissions"], 2);
    assert_ne!(wake["requestId"], "original-wake");
    f.pass(&source).await.unwrap();
    assert_eq!(f.wake().await["state"], "completed");
    assert_eq!(source.captures.load(Ordering::SeqCst), 1);
    assert_eq!(source.wake_calls.load(Ordering::SeqCst), 1);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn unanswered_wake_on_a_running_machine_is_accepted_without_replay() {
    let f = Fixture::start(stuck_wake()).await;
    let source = Source::new(MachineState::Running, vec![]);
    f.pass(&source).await.unwrap();
    let wake = f.wake().await;
    assert_eq!(wake["state"], "completed");
    assert_eq!(wake["requestId"], "original-wake");
    assert_eq!(source.wake_calls.load(Ordering::SeqCst), 0);
    assert_eq!(source.captures.load(Ordering::SeqCst), 1);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn wakes_that_never_happen_stop_with_attention_instead_of_looping() {
    let f = Fixture::start(Value::Null).await;
    let source = Source::new(
        MachineState::Paused,
        vec![false; EFFECT_MAX_SUBMISSIONS as usize],
    );
    let mut codes = Vec::new();
    for submission in 1..=EFFECT_MAX_SUBMISSIONS {
        codes.push(f.pass(&source).await.unwrap_err().code);
        assert_eq!(
            source.wake_calls.load(Ordering::SeqCst),
            submission as usize
        );
        let wake = f.wake().await;
        assert_eq!(wake["state"], "uncertain");
        assert_eq!(wake["submissions"], submission);
        assert_eq!(wake["error"], "outcome-unknown");
        // Waiting inside the settle window neither replays nor fails.
        f.pass(&source).await.unwrap();
        assert_eq!(
            source.wake_calls.load(Ordering::SeqCst),
            submission as usize
        );
        f.age(EFFECT_SETTLE_MS).await;
    }
    assert!(
        codes
            .iter()
            .all(|c| *c == "archive-provider-operation-failed")
    );
    assert_eq!(
        f.pass(&source).await.unwrap_err().code,
        "archive-provider-wake-failed"
    );
    assert_eq!(
        source.wake_calls.load(Ordering::SeqCst),
        EFFECT_MAX_SUBMISSIONS as usize
    );
    assert_eq!(f.wake().await["state"], "not-applied");
    let (state, error): (String, Option<String>) =
        sqlx::query_as("SELECT state,last_error_code FROM zuse_rust_jobs")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(state, "failed", "the job stops instead of re-polling");
    assert_eq!(error.as_deref(), Some("archive-provider-wake-failed"));
    let status: String = sqlx::query_scalar(
        "SELECT status_code FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(status, "archive-provider-wake-failed");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn a_wake_stuck_between_states_stops_with_attention_after_the_wait_limit() {
    let f = Fixture::start(stuck_wake()).await;
    let source = Source::new(MachineState::Unknown("waking".into()), vec![]);
    f.pass(&source).await.unwrap();
    f.age(EFFECT_SETTLE_MS).await;
    // Not provably unchanged: keep waiting, never replay.
    f.pass(&source).await.unwrap();
    assert_eq!(source.wake_calls.load(Ordering::SeqCst), 0);
    f.age(EFFECT_WAIT_LIMIT_MS).await;
    assert_eq!(
        f.pass(&source).await.unwrap_err().code,
        "archive-provider-wake-unconfirmed"
    );
    assert_eq!(source.wake_calls.load(Ordering::SeqCst), 0);
    let wake = f.wake().await;
    assert_eq!(wake["state"], "uncertain", "identity kept for the retry");
    assert_eq!(wake["requestId"], "original-wake");
    let state: String = sqlx::query_scalar("SELECT state FROM zuse_rust_jobs")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(state, "failed");
    f.finish().await;
}

/// The source woke for capture earlier in this operation.
fn woken() -> Value {
    json!({"state":"completed","receipt":{"resourceId":"machine-bob","operationId":null,"desired":"running"},"requestId":"capture-wake","submissions":1})
}
impl Fixture {
    async fn capture(&self) -> Value {
        sqlx::query_scalar("SELECT provider_effects->'capture' FROM zuse_rust_recovery_operations")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    /// Pretend the capture helper was launched `ms` ago.
    async fn launched_ago(&self, ms: i64) {
        sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,'{capture,launchedAtMs}',to_jsonb($1::bigint))").bind(clock_ms() - ms).execute(&self.pool).await.unwrap();
    }
    async fn job(&self) -> (String, Option<String>) {
        sqlx::query_as("SELECT state,last_error_code FROM zuse_rust_jobs")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn status_code(&self) -> String {
        sqlx::query_scalar(
            "SELECT status_code FROM api_cloud_workspaces WHERE workspace_id='ws-bob'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
    /// What Update computer Retry does to the recovery job.
    async fn retry(&self) {
        sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=0,last_error_code=NULL")
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn capture_is_launched_once_and_then_only_polled() {
    let f = Fixture::start(woken()).await;
    let source = Source::new(MachineState::Running, vec![]);
    for _ in 0..5 {
        f.pass(&source).await.unwrap();
    }
    assert_eq!(
        source.captures.load(Ordering::SeqCst),
        1,
        "relaunching would touch the guest data directory under the capture"
    );
    let capture = f.capture().await;
    assert_eq!(capture["state"], "launched");
    assert_eq!(capture["attempt"], 1);
    // A published archive keeps waiting for the manifest to advance the phase.
    *source.status.lock().unwrap() = CaptureStatus::Uploaded;
    f.pass(&source).await.unwrap();
    // Not running and no result yet, inside the start grace: still waiting.
    *source.status.lock().unwrap() = CaptureStatus::Idle;
    f.pass(&source).await.unwrap();
    assert_eq!(source.captures.load(Ordering::SeqCst), 1);
    assert_eq!(f.job().await.0, "pending");
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn a_failed_capture_helper_needs_attention_and_retry_launches_a_fresh_attempt() {
    let f = Fixture::start(woken()).await;
    let source = Source::new(MachineState::Running, vec![]);
    f.pass(&source).await.unwrap();
    *source.status.lock().unwrap() = CaptureStatus::Failed;
    assert_eq!(
        f.pass(&source).await.unwrap_err().code,
        "archive-capture-failed"
    );
    assert_eq!(
        f.job().await,
        ("failed".into(), Some("archive-capture-failed".into())),
        "the job stops instead of polling a dead helper"
    );
    assert_eq!(f.status_code().await, "archive-capture-failed");
    assert_eq!(f.capture().await["state"], "ended");
    assert_eq!(source.captures.load(Ordering::SeqCst), 1);
    f.retry().await;
    *source.status.lock().unwrap() = CaptureStatus::Running;
    f.pass(&source).await.unwrap();
    assert_eq!(source.captures.load(Ordering::SeqCst), 2);
    let capture = f.capture().await;
    assert_eq!(capture["state"], "launched");
    assert_eq!(capture["attempt"], 2);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn a_helper_that_never_starts_or_never_finishes_is_bounded() {
    let f = Fixture::start(woken()).await;
    let source = Source::new(MachineState::Running, vec![]);
    f.pass(&source).await.unwrap();
    *source.status.lock().unwrap() = CaptureStatus::Idle;
    f.launched_ago(CAPTURE_START_GRACE_MS + 1).await;
    assert_eq!(
        f.pass(&source).await.unwrap_err().code,
        "archive-capture-failed"
    );
    f.retry().await;
    *source.status.lock().unwrap() = CaptureStatus::Running;
    f.pass(&source).await.unwrap();
    assert_eq!(source.captures.load(Ordering::SeqCst), 2);
    f.launched_ago(CAPTURE_LIMIT_MS + 1).await;
    assert_eq!(
        f.pass(&source).await.unwrap_err().code,
        "archive-capture-timeout"
    );
    assert_eq!(
        f.job().await,
        ("failed".into(), Some("archive-capture-timeout".into()))
    );
    assert_eq!(f.status_code().await, "archive-capture-timeout");
    assert_eq!(source.captures.load(Ordering::SeqCst), 2);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn a_launch_renews_a_capability_that_would_expire_during_the_upload() {
    let f = Fixture::start(woken()).await;
    let soon = clock_ms() + 5 * 60_000;
    sqlx::query("UPDATE zuse_rust_recovery_operations SET expires_at=$1")
        .bind(soon)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{recoveryArchive,expiresAtMs}',to_jsonb($1::bigint))").bind(soon).execute(&f.pool).await.unwrap();
    let source = Source::new(MachineState::Running, vec![]);
    f.pass(&source).await.unwrap();
    let expires: i64 = sqlx::query_scalar("SELECT expires_at FROM zuse_rust_recovery_operations")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(expires >= clock_ms() + CAPTURE_LIMIT_MS);
    assert_eq!(source.captures.load(Ordering::SeqCst), 1);
    f.finish().await;
}

#[test]
fn guest_capture_status_reads_the_helpers_status_directory() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("archive-original");
    std::fs::create_dir(&directory).unwrap();
    let read = || {
        let command =
            capture_status_command("env", root.path().to_str().unwrap(), "archive-original");
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .output()
            .unwrap();
        capture_status(&crate::providers::ExecResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.status.code().unwrap_or(-1),
        })
        .unwrap()
    };
    assert_eq!(read(), CaptureStatus::Idle);
    std::fs::write(directory.join("pid"), "999999999\n").unwrap();
    assert_eq!(
        read(),
        CaptureStatus::Idle,
        "a stale pid is not a live helper"
    );
    std::fs::write(directory.join("pid"), format!("{}\n", std::process::id())).unwrap();
    assert_eq!(read(), CaptureStatus::Running);
    std::fs::write(directory.join("failed"), "archive-upload-failed\n").unwrap();
    assert_eq!(read(), CaptureStatus::Failed);
    std::fs::write(directory.join("uploaded"), "workspace_archive_uploaded\n").unwrap();
    assert_eq!(read(), CaptureStatus::Uploaded);
    assert!(
        capture_status(&crate::providers::ExecResult {
            stdout: b"running\n".to_vec(),
            stderr: vec![],
            exit_code: 1,
        })
        .is_err()
    );
}

impl Fixture {
    /// The capture uploaded and the operation is verifying.
    async fn verifying(&self, effects: Value) {
        let manifest = json!({"version":1,"operationId":"archive-original","workspaceId":"ws-bob","chatId":"chat-bob","sessionId":"session-bob","chunks":["a".repeat(64)],"archiveSha256":"b".repeat(64)});
        sqlx::query("UPDATE zuse_rust_recovery_operations SET phase='verifying',helper_stopped=true,provider_effects=$1,manifest=$2")
            .bind(effects)
            .bind(&manifest)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(jsonb_set(request_config,'{recoveryArchive,phase}','\"verifying\"'),'{recoveryArchive,manifest}',$1) WHERE workspace_id='ws-bob'").bind(&manifest).execute(&self.pool).await.unwrap();
    }
    async fn claim(&self) -> jobs::Job {
        sqlx::query("UPDATE zuse_rust_jobs SET due_at=0,lease_expires_at=0 WHERE state<>'failed'")
            .execute(&self.pool)
            .await
            .unwrap();
        jobs::claim(&self.pool, JOB_KIND, "workspace", clock_ms(), LEASE)
            .await
            .unwrap()
            .expect("recovery job is claimable")
    }
    async fn verification(&self) -> Value {
        sqlx::query_scalar(
            "SELECT provider_effects->'verification' FROM zuse_rust_recovery_operations",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[test]
fn verification_failures_are_named_and_classified() {
    let full = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::StorageFull));
    assert_eq!(verification_reason(&full), "recovery_scratch_insufficient");
    assert_eq!(
        verification_reason(&anyhow::anyhow!("recovery_chunk_authentication_failed")),
        "recovery_chunk_authentication_failed"
    );
    assert_eq!(
        verification_reason(&anyhow::anyhow!("free text from /some/path")),
        "unclassified"
    );
    assert_eq!(
        classify_verification("recovery_scratch_insufficient"),
        VerificationFailure::ScratchFull
    );
    assert_eq!(
        classify_verification("archive_object_unavailable"),
        VerificationFailure::Transient
    );
    assert_eq!(
        classify_verification("recovery_archive_credentials_forbidden"),
        VerificationFailure::Rejected
    );
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn verification_failures_are_bounded_and_surfaced_with_their_reason() {
    for (reason, code) in [
        (
            "recovery_scratch_insufficient",
            "archive-verification-scratch-full",
        ),
        (
            "recovery_archive_checksum_mismatch",
            "archive-verification-rejected",
        ),
    ] {
        let f = Fixture::start(woken()).await;
        f.verifying(json!({"wake":woken()})).await;
        let job = f.claim().await;
        assert_eq!(
            f.control
                .recovery_verification_failed(&job, reason)
                .await
                .unwrap()
                .code,
            code
        );
        assert_eq!(f.job().await, ("failed".into(), Some(code.into())));
        assert_eq!(f.status_code().await, code);
        assert_eq!(f.verification().await["lastReason"], reason);
        f.finish().await;
    }
    // A transient failure is retried, then needs attention.
    let f = Fixture::start(woken()).await;
    f.verifying(json!({"wake":woken()})).await;
    for attempt in 1..VERIFICATION_MAX_ATTEMPTS {
        let job = f.claim().await;
        assert_eq!(
            f.control
                .recovery_verification_failed(&job, "archive_object_unavailable")
                .await
                .unwrap()
                .code,
            "archive-verification-unavailable"
        );
        assert_eq!(f.verification().await["attempts"], attempt);
        // The worker's own retry bookkeeping.
        sqlx::query("UPDATE zuse_rust_jobs SET state='pending',lease_token=NULL")
            .execute(&f.pool)
            .await
            .unwrap();
    }
    let job = f.claim().await;
    assert_eq!(
        f.control
            .recovery_verification_failed(&job, "archive_object_unavailable")
            .await
            .unwrap()
            .code,
        "archive-verification-failed"
    );
    assert_eq!(f.job().await.0, "failed");
    assert_eq!(
        f.verification().await["attempts"],
        0,
        "Retry starts a fresh count"
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn a_source_woken_after_its_pause_completed_is_paused_again() {
    let f = Fixture::start(woken()).await;
    let paused = json!({"state":"completed","receipt":{"resourceId":"machine-bob","operationId":null,"desired":"sleeping"},"requestId":"first-pause","submissions":1});
    f.verifying(json!({"wake":woken(),"pause":paused})).await;
    let source = Source::new(MachineState::Running, vec![]);
    let job = f.claim().await;
    assert!(
        !f.control
            .recovery_prepare_verification(&source, &job)
            .await
            .unwrap()
    );
    assert_eq!(
        source.pauses.load(Ordering::SeqCst),
        1,
        "a spent pause must not leave the stage waiting forever"
    );
    let pause: Value =
        sqlx::query_scalar("SELECT provider_effects->'pause' FROM zuse_rust_recovery_operations")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_ne!(pause["requestId"], "first-pause");
    // Asleep again: verification may proceed once the new pause completes.
    let job = f.claim().await;
    assert!(
        f.control
            .recovery_prepare_verification(&source, &job)
            .await
            .unwrap()
    );
    assert_eq!(source.pauses.load(Ordering::SeqCst), 1);
    f.finish().await;
}
