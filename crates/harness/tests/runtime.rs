use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::{Barrier, Notify};
use zuse_harness::*;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
fn output(text: &str, calls: Vec<Call>) -> ModelOutput {
    let mut content = vec![json!({"type":"text","text":text})];
    content.extend(
        calls.iter().map(
            |c| json!({"type":"tool-call","toolCallId":c.id,"toolName":c.name,"input":c.input}),
        ),
    );
    ModelOutput {
        messages: vec![Message {
            role: Role::Assistant,
            content: Value::Array(content),
            metadata: BTreeMap::new(),
        }],
        calls,
        text: text.into(),
        usage: Usage::default(),
    }
}
fn call(id: &str, name: &str, input: Value) -> Call {
    Call {
        id: id.into(),
        name: name.into(),
        input,
    }
}
struct Fixture {
    store: Mutex<Option<Checkpoint>>,
    responses: Mutex<VecDeque<ModelOutput>>,
    events: Mutex<Vec<Event>>,
    models: Mutex<Vec<(String, Option<String>)>>,
    executions: AtomicUsize,
    mode: AtomicUsize,
    barrier: Barrier,
    blocked: Notify,
    release: Notify,
}
impl Fixture {
    fn new(responses: Vec<ModelOutput>) -> Arc<Self> {
        Arc::new(Self {
            store: Mutex::new(None),
            responses: Mutex::new(responses.into()),
            events: Mutex::new(vec![]),
            models: Mutex::new(vec![]),
            executions: AtomicUsize::new(0),
            mode: AtomicUsize::new(0),
            barrier: Barrier::new(2),
            blocked: Notify::new(),
            release: Notify::new(),
        })
    }
}
#[async_trait]
impl Host for Fixture {
    fn tools(&self) -> Tools {
        BTreeMap::from([(
            "edit".into(),
            ToolDefinition {
                description: "Edit fixture".into(),
                parameters: json!({"type":"object"}),
                category: ToolCategory::Edit,
            },
        )])
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        Ok(lock(&self.store).clone())
    }
    async fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        if self.mode.load(Ordering::Relaxed) == 7 && !state.reviews.is_empty()
            || self.mode.load(Ordering::Relaxed) == 9 && !state.consumed_notifications.is_empty()
            || self.mode.load(Ordering::Relaxed) == 10 && state.model == "unsaved-model"
            || self.mode.load(Ordering::Relaxed) == 11 && !state.cancelled_notifications.is_empty()
            || self.mode.load(Ordering::Relaxed) == 12 && state.notifications.contains("first")
            || self.mode.load(Ordering::Relaxed) == 13
        {
            return Err(Error::Storage);
        }
        state.validate()?;
        *lock(&self.store) = Some(state.clone());
        Ok(())
    }
    async fn instructions(&self) -> Result<String, Error> {
        Ok("Preserve user requirements.".into())
    }
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        lock(&self.models).push((input.model.clone(), input.reasoning.clone()));
        if self.mode.load(Ordering::Relaxed) == 4 && input.tools.is_empty() {
            return Ok(output(
                "Preserve the original request and unfinished work.",
                vec![],
            ));
        }
        if self.mode.load(Ordering::Relaxed) == 2 && input.agent_id != "root" {
            self.barrier.wait().await;
            return Ok(output(&format!("{} finished", input.agent_id), vec![]));
        }
        if self.mode.load(Ordering::Relaxed) == 8 && input.agent_id != "root" {
            self.blocked.notify_one();
            self.release.notified().await;
            return Ok(output("child survived", vec![]));
        }
        if self.mode.load(Ordering::Relaxed) == 3 {
            self.blocked.notify_one();
            self.release.notified().await;
        }
        Ok(lock(&self.responses)
            .pop_front()
            .expect("unexpected physical request"))
    }
    async fn execute(&self, _: &str, _: Value, _: ToolContext) -> Result<ToolOutput, Error> {
        self.executions.fetch_add(1, Ordering::Relaxed);
        if self.mode.load(Ordering::Relaxed) == 1 {
            self.blocked.notify_one();
            self.release.notified().await;
        }
        Ok("edited".to_owned().into())
    }
    fn notify(&self, event: Event) {
        lock(&self.events).push(event);
    }
}
async fn engine(f: Arc<Fixture>) -> Engine {
    Engine::open("root", Options::new("model"), f)
        .await
        .unwrap()
}

#[tokio::test]
async fn unknown_tool_outcome_is_preserved_and_never_automatically_replayed() {
    let f = Fixture::new(vec![output(
        "editing",
        vec![call("edit-1", "edit", json!({"path":"file"}))],
    )]);
    f.mode.store(1, Ordering::Relaxed);
    let e = engine(f.clone()).await;
    let running = {
        let e = e.clone();
        tokio::spawn(async move { e.send("work".into(), vec![]).await })
    };
    f.blocked.notified().await;
    e.interrupt(None).await.unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(f.executions.load(Ordering::Relaxed), 1);
    let before = e.snapshot().await;
    assert!(matches!(
        before.tools["root:edit-1"].status,
        ReceiptStatus::Started
    ));
    let recovered = engine(f.clone()).await;
    recovered
        .send("Continue after inspection".into(), vec![])
        .await
        .unwrap();
    assert_eq!(f.executions.load(Ordering::Relaxed), 1);
    assert_eq!(
        recovered.snapshot().await.agent("root").unwrap().status,
        Status::Interrupted
    );
    assert!(
        lock(&f.events)
            .iter()
            .any(|e| matches!(e,Event::Paused{reason,..} if reason.contains("unknown outcome")))
    );
    f.mode.store(0, Ordering::Relaxed);
    lock(&f.responses).push_back(output("reviewed and completed", vec![]));
    recovered
        .resolve_unknown("root", "edit-1", RecoveryDecision::RetryAfterInspection)
        .await
        .unwrap();
    assert_eq!(f.executions.load(Ordering::Relaxed), 1);
    recovered
        .send("Retry after reviewing the outcome".into(), vec![])
        .await
        .unwrap();
    assert_eq!(f.executions.load(Ordering::Relaxed), 2);
    assert_eq!(recovered.snapshot().await.reviews.len(), 1);
}

#[tokio::test]
async fn committed_tool_receipt_recovers_without_a_second_side_effect() {
    let f = Fixture::new(vec![output("done", vec![])]);
    let e = engine(f.clone()).await;
    let mut checkpoint = e.snapshot().await;
    let c = call("saved", "edit", json!({}));
    checkpoint.agent_mut("root").unwrap().pending_calls = vec![c];
    checkpoint.tools.insert(
        "root:saved".into(),
        Receipt {
            status: ReceiptStatus::Completed,
            output: Some("already edited".to_owned().into()),
            signature: None,
        },
    );
    *lock(&f.store) = Some(checkpoint);
    let recovered = engine(f.clone()).await;
    recovered.send("resume".into(), vec![]).await.unwrap();
    assert_eq!(f.executions.load(Ordering::Relaxed), 0);
    let state = recovered.snapshot().await;
    assert!(state.agent("root").unwrap().pending_calls.is_empty());
    assert_eq!(state.agent("root").unwrap().status, Status::Completed);
    assert!(
        state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .any(|m| m.role == Role::Tool)
    );
}

#[tokio::test]
async fn child_requests_run_concurrently_and_root_synthesizes_after_delivery() {
    let f = Fixture::new(vec![
        output(
            "delegate",
            vec![
                call(
                    "spawn-a",
                    "spawn_agent",
                    json!({"name":"a","message":"first","inherit":false}),
                ),
                call(
                    "spawn-b",
                    "spawn_agent",
                    json!({"name":"b","message":"second","inherit":false}),
                ),
            ],
        ),
        output("awaiting workers", vec![]),
        output("all delivered", vec![]),
    ]);
    f.mode.store(2, Ordering::Relaxed);
    let e = engine(f.clone()).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        e.send("Do both".into(), vec![]),
    )
    .await
    .unwrap()
    .unwrap();
    let state = e.snapshot().await;
    assert_eq!(state.agents.len(), 3);
    assert!(state.agents.iter().all(|a| a.status == Status::Completed));
    assert!(
        state
            .agents
            .iter()
            .filter(|a| a.parent_id.is_some())
            .all(|a| a.completed_delivery)
    );
    assert_eq!(state.agent("root").unwrap().summary, "all delivered");
    assert_eq!(state.requests, 5);
}

#[tokio::test]
async fn read_only_policy_blocks_edits_before_host_execution() {
    let f = Fixture::new(vec![
        output("try", vec![call("edit-1", "edit", json!({}))]),
        output("explained restriction", vec![]),
    ]);
    let e = engine(f.clone()).await;
    e.set_permission_mode(PermissionMode::Plan);
    e.send("inspect".into(), vec![]).await.unwrap();
    assert_eq!(f.executions.load(Ordering::Relaxed), 0);
    assert!(lock(&f.events).iter().any(|e|matches!(e,Event::ToolResult{output:ToolOutput::Text(text),..} if text.contains("read-only"))));
}

#[tokio::test]
async fn steering_is_durable_and_delivery_ids_are_deduplicated() {
    let f = Fixture::new(vec![output("initial", vec![]), output("steered", vec![])]);
    f.mode.store(3, Ordering::Relaxed);
    let e = engine(f.clone()).await;
    let work = {
        let e = e.clone();
        tokio::spawn(async move { e.send("start".into(), vec![]).await })
    };
    f.blocked.notified().await;
    e.enqueue(
        "delivery-1".into(),
        "root",
        "new constraint".into(),
        vec![],
        false,
    )
    .await
    .unwrap();
    e.enqueue(
        "delivery-1".into(),
        "root",
        "new constraint".into(),
        vec![],
        false,
    )
    .await
    .unwrap();
    assert!(!lock(&f.events).iter().any(|event| matches!(event, Event::MailConsumed { delivery_id, .. } if delivery_id == "delivery-1")));
    f.mode.store(0, Ordering::Relaxed);
    f.release.notify_one();
    work.await.unwrap().unwrap();
    let state = e.snapshot().await;
    assert_eq!(
        state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .filter(|m| m.content == "new constraint")
            .count(),
        1
    );
    assert_eq!(state.agent("root").unwrap().summary, "steered");
    assert_eq!(
        state
            .consumed_notifications
            .get("delivery-1")
            .map(String::as_str),
        Some("root")
    );
    assert_eq!(lock(&f.events).iter().filter(|event| matches!(event, Event::MailConsumed { delivery_id, .. } if delivery_id == "delivery-1")).count(), 1);
    assert!(!e.acknowledge_consumed_delivery("unknown").await.unwrap());
    let event_count = lock(&f.events).len();
    f.mode.store(9, Ordering::Relaxed);
    assert!(e.acknowledge_consumed_delivery("delivery-1").await.is_err());
    assert_eq!(lock(&f.events).len(), event_count);
    f.mode.store(0, Ordering::Relaxed);
    // Losing the session acknowledgement across restart must not replay mail.
    let recovered = engine(f.clone()).await;
    assert!(
        recovered
            .acknowledge_consumed_delivery("delivery-1")
            .await
            .unwrap()
    );
    recovered
        .enqueue(
            "delivery-1".into(),
            "root",
            "must not replay".into(),
            vec![],
            false,
        )
        .await
        .unwrap();
    assert!(
        recovered
            .snapshot()
            .await
            .agent("root")
            .unwrap()
            .mailbox
            .is_empty()
    );
    assert_eq!(lock(&f.events).iter().filter(|event| matches!(event, Event::MailConsumed { delivery_id, .. } if delivery_id == "delivery-1")).count(), 3);
}

#[tokio::test]
async fn request_budget_is_shared_and_requires_explicit_continuation() {
    let f = Fixture::new(vec![
        output("tool", vec![call("edit-1", "edit", json!({}))]),
        output("finished", vec![]),
    ]);
    let mut options = Options::new("model");
    options.max_requests = 1;
    let e = Engine::open("root", options, f.clone()).await.unwrap();
    e.send("work".into(), vec![]).await.unwrap();
    assert_eq!(e.snapshot().await.requests, 1);
    assert_eq!(
        e.snapshot().await.agent("root").unwrap().status,
        Status::Interrupted
    );
    e.send("continue".into(), vec![]).await.unwrap();
    assert_eq!(
        e.snapshot().await.agent("root").unwrap().status,
        Status::Completed
    );
}

#[tokio::test]
async fn unchanged_model_updates_runtime_limits_without_rewriting_history() {
    let f = Fixture::new(vec![
        output("tool", vec![call("edit-1", "edit", json!({}))]),
        output("finished", vec![]),
    ]);
    let e = engine(f.clone()).await;
    assert_eq!(e.root_id().await, "root");
    let mut options = Options::new("model");
    options.max_requests = 1;
    f.mode.store(13, Ordering::Relaxed);
    e.reconfigure(options).await.unwrap();
    f.mode.store(0, Ordering::Relaxed);
    e.send("work".into(), vec![]).await.unwrap();
    let state = e.snapshot().await;
    assert_eq!(state.requests, 1);
    assert_eq!(state.agent("root").unwrap().status, Status::Interrupted);
    assert_eq!(f.executions.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn checkpoint_graph_corruption_is_rejected_without_resetting_history() {
    let f = Fixture::new(vec![]);
    let e = engine(f.clone()).await;
    let mut state = e.snapshot().await;
    state
        .agent_mut("root")
        .unwrap()
        .history
        .push(Message::text(Role::User, "Buffered checkpoint: λ\n".repeat(10_000)).into());
    let directory = tempfile::tempdir().unwrap();
    let store =
        zuse_harness::store::FileStore::new(directory.path().join("session/checkpoint.json"));
    store.save(&state).unwrap();
    assert_eq!(
        serde_json::to_vec(&store.load().unwrap().unwrap()).unwrap(),
        serde_json::to_vec(&state).unwrap(),
    );
    assert_eq!(
        std::fs::read_dir(store.path().parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(store.path().parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let mut broken = state.clone();
    broken.agents.push(broken.agents[0].clone());
    assert!(matches!(store.save(&broken), Err(Error::InvalidCheckpoint)));
    assert_eq!(store.load().unwrap().unwrap().agents.len(), 1);
    std::fs::write(store.path(), "{corrupt").unwrap();
    assert!(matches!(store.load(), Err(Error::InvalidCheckpoint)));
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), "{corrupt");
}

#[tokio::test]
async fn compaction_preserves_original_task_and_full_history_with_a_durable_boundary() {
    let f = Fixture::new(vec![output("continued safely", vec![])]);
    let initial = engine(f.clone()).await;
    let mut state = initial.snapshot().await;
    state.agent_mut("root").unwrap().history.push(
        Message::text(
            Role::User,
            "Finish the complete request; retain pending work.",
        )
        .into(),
    );
    for _ in 0..20 {
        state
            .agent_mut("root")
            .unwrap()
            .history
            .push(Message::text(Role::User, "old context ".repeat(100)).into());
        state
            .agent_mut("root")
            .unwrap()
            .history
            .push(Message::text(Role::Assistant, "prior outcome ".repeat(100)).into());
    }
    let original_len = state.agent("root").unwrap().history.len();
    *lock(&f.store) = Some(state);
    f.mode.store(4, Ordering::Relaxed);
    let mut options = Options::new("model");
    options.window_tokens = 2000;
    let resumed = Engine::open("root", options, f.clone()).await.unwrap();
    resumed.send("Continue".into(), vec![]).await.unwrap();
    let state = resumed.snapshot().await;
    let root = state.agent("root").unwrap();
    assert!(root.history.len() > original_len);
    assert!(root.checkpoint.is_some());
    let rendered = zuse_harness::prompt::render(&root.history, root.checkpoint.as_ref());
    assert!(
        rendered
            .iter()
            .any(|m| m.content == "Finish the complete request; retain pending work.")
    );
    assert!(lock(&f.events).iter().any(|e|matches!(e,Event::Compaction{status,before,after:Some(after),..} if status=="completed" && after<before)));
    assert_eq!(state.requests, 2);
    assert_eq!(root.status, Status::Completed);
}

#[tokio::test]
async fn recovery_approval_that_cannot_be_saved_does_not_allow_replay() {
    let f = Fixture::new(vec![]);
    let e = engine(f.clone()).await;
    let mut state = e.snapshot().await;
    state.agent_mut("root").unwrap().pending_calls = vec![call("uncertain", "edit", json!({}))];
    state.tools.insert(
        "root:uncertain".into(),
        Receipt {
            status: ReceiptStatus::Started,
            output: None,
            signature: None,
        },
    );
    *lock(&f.store) = Some(state);
    let recovered = engine(f.clone()).await;
    f.mode.store(7, Ordering::Relaxed);
    assert!(matches!(
        recovered
            .resolve_unknown("root", "uncertain", RecoveryDecision::RetryAfterInspection)
            .await,
        Err(Error::Storage)
    ));
    f.mode.store(0, Ordering::Relaxed);
    recovered.send("Continue".into(), vec![]).await.unwrap();
    assert_eq!(f.executions.load(Ordering::Relaxed), 0);
    assert!(recovered.snapshot().await.reviews.is_empty());
    assert!(matches!(
        recovered.snapshot().await.tools["root:uncertain"].status,
        ReceiptStatus::Started
    ));
}

#[tokio::test]
async fn stopping_a_turn_retains_child_work_and_allows_root_continuation() {
    let f = Fixture::new(vec![
        output(
            "delegate",
            vec![call(
                "spawn",
                "spawn_agent",
                json!({"name":"worker","message":"continue independently","inherit":false}),
            )],
        ),
        output("waiting", vec![]),
        output("continued", vec![]),
        output("synthesized", vec![]),
    ]);
    f.mode.store(8, Ordering::Relaxed);
    let e = engine(f.clone()).await;
    let first = {
        let e = e.clone();
        tokio::spawn(async move { e.send("start".into(), vec![]).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), f.blocked.notified())
        .await
        .unwrap();
    e.stop_turn().await.unwrap();
    first.await.unwrap().unwrap();
    let state = e.snapshot().await;
    assert_eq!(state.agent("root").unwrap().status, Status::Interrupted);
    assert!(
        state
            .agents
            .iter()
            .any(|a| a.parent_id.is_some() && a.status == Status::Running)
    );
    assert_eq!(e.background_count(), 1);
    let mut options = Options::new("next-model");
    options.reasoning = Some("high".into());
    options.window_tokens = 10000;
    options.effective_window_percent = 95.5;
    e.reconfigure(options.clone()).await.unwrap();
    options.subagents = false;
    assert!(e.reconfigure(options).await.is_err());
    let next = {
        let e = e.clone();
        tokio::spawn(async move { e.send("new instruction".into(), vec![]).await })
    };
    // Wait for the root to accept the next turn while its child is still blocked.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if e.snapshot()
                .await
                .agent("root")
                .unwrap()
                .history
                .iter()
                .any(|m| m.content == "new instruction")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!next.is_finished());
    f.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(3), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let state = e.snapshot().await;
    assert_eq!(state.agent("root").unwrap().status, Status::Completed);
    assert!(
        state
            .agents
            .iter()
            .any(|a| a.parent_id.is_some() && a.summary == "child survived")
    );
    assert_eq!(state.model, "next-model");
    assert!(lock(&f.models).iter().any(|(model, reasoning)| model == "next-model" && reasoning.as_deref() == Some("high")));
    assert!(lock(&f.events).iter().any(|event| matches!(event, Event::Context { agent_id, window_tokens: 9550, .. } if agent_id == "root")));
    e.close().await.unwrap();
}

#[tokio::test]
async fn failed_mail_commit_never_acknowledges_steering_and_recovery_delivers_it_once() {
    let f = Fixture::new(vec![output("initial", vec![]), output("recovered", vec![])]);
    f.mode.store(3, Ordering::Relaxed);
    let e = engine(f.clone()).await;
    let work = {
        let e = e.clone();
        tokio::spawn(async move { e.send("start".into(), vec![]).await })
    };
    f.blocked.notified().await;
    e.enqueue(
        "durable-steer".into(),
        "root",
        "keep this constraint".into(),
        vec![],
        false,
    )
    .await
    .unwrap();
    f.mode.store(9, Ordering::Relaxed);
    f.release.notify_one();
    work.await.unwrap().unwrap();
    assert!(
        !lock(&f.events)
            .iter()
            .any(|event| matches!(event, Event::MailConsumed { .. }))
    );
    assert_eq!(
        lock(&f.store)
            .as_ref()
            .unwrap()
            .agent("root")
            .unwrap()
            .mailbox
            .len(),
        1
    );
    e.close().await.unwrap();
    f.mode.store(0, Ordering::Relaxed);
    let recovered = engine(f.clone()).await;
    recovered.send(String::new(), vec![]).await.unwrap();
    let state = recovered.snapshot().await;
    assert_eq!(
        state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .filter(|m| m.content == "keep this constraint")
            .count(),
        1
    );
    assert_eq!(lock(&f.events).iter().filter(|event| matches!(event, Event::MailConsumed { delivery_id, .. } if delivery_id == "durable-steer")).count(), 1);
    f.mode.store(10, Ordering::Relaxed);
    assert!(matches!(
        recovered.reconfigure(Options::new("unsaved-model")).await,
        Err(Error::Storage)
    ));
    assert_eq!(recovered.snapshot().await.model, "model");
    recovered.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_mail_is_durable_and_failed_cancellation_restores_its_original_order() {
    let f = Fixture::new(vec![output("continued", vec![])]);
    let e = engine(f.clone()).await;
    for (id, text) in [("a", "first"), ("b", "second"), ("c", "third")] {
        e.enqueue(id.into(), "root", text.into(), vec![], false)
            .await
            .unwrap();
    }
    f.mode.store(11, Ordering::Relaxed);
    assert!(matches!(
        e.cancel_deliveries(&["a".into(), "c".into()]).await,
        Err(Error::Storage)
    ));
    assert_eq!(
        e.snapshot()
            .await
            .agent("root")
            .unwrap()
            .mailbox
            .iter()
            .map(|mail| mail.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    f.mode.store(0, Ordering::Relaxed);
    e.cancel_deliveries(&["a".into(), "c".into()])
        .await
        .unwrap();
    e.close().await.unwrap();
    let recovered = engine(f.clone()).await;
    assert!(matches!(
        recovered
            .enqueue("a".into(), "root", "must not return".into(), vec![], false)
            .await,
        Err(Error::Interrupted)
    ));
    recovered.send(String::new(), vec![]).await.unwrap();
    let state = recovered.snapshot().await;
    assert!(state.cancelled_notifications.contains("a"));
    assert!(
        state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .any(|message| message.content == "second")
    );
    assert!(
        !state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .any(|message| message.content == "first" || message.content == "third")
    );
    assert!(
        recovered
            .cancel_deliveries(&["b".into()])
            .await
            .unwrap()
            .is_empty()
    );
    recovered.close().await.unwrap();
}

#[tokio::test]
async fn identified_initial_send_keeps_pending_content_and_never_repeats_consumed_work() {
    let f = Fixture::new(vec![output("finished", vec![])]);
    let e = engine(f.clone()).await;
    e.enqueue(
        "initial".into(),
        "root",
        "Original pending prompt".into(),
        vec![],
        false,
    )
    .await
    .unwrap();
    let recovered = engine(f.clone()).await;
    assert_eq!(
        recovered
            .send_delivery_with_cancel(
                "initial".into(),
                "Changed retry".into(),
                vec![],
                &tokio_util::sync::CancellationToken::new()
            )
            .await
            .unwrap(),
        SendOutcome::Started
    );
    let models = lock(&f.models).len();
    let state = recovered.snapshot().await;
    assert_eq!(
        state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .filter(|m| m.content == "Original pending prompt")
            .count(),
        1
    );
    assert!(
        !state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .any(|m| m.content == "Changed retry")
    );
    assert_eq!(
        state
            .consumed_notifications
            .get("initial")
            .map(String::as_str),
        Some("root")
    );
    for current in [recovered.clone(), engine(f.clone()).await] {
        assert_eq!(
            current
                .send_delivery_with_cancel(
                    "initial".into(),
                    "Must not execute".into(),
                    vec![],
                    &tokio_util::sync::CancellationToken::new()
                )
                .await
                .unwrap(),
            SendOutcome::AlreadyConsumed
        );
        assert_eq!(lock(&f.models).len(), models);
        assert!(
            current
                .snapshot()
                .await
                .agent("root")
                .unwrap()
                .mailbox
                .is_empty()
        );
    }
}

#[tokio::test]
async fn initial_delivery_waits_for_durable_admission_and_cancellation_prevents_generation() {
    let f = Fixture::new(vec![output("finished", vec![])]);
    let e = engine(f.clone()).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        e.send_delivery_with_cancel(
            "cancelled-before-admission".into(),
            "Not accepted".into(),
            vec![],
            &cancel
        )
        .await,
        Err(Error::Interrupted)
    ));
    assert!(lock(&f.models).is_empty());
    assert!(
        !e.snapshot()
            .await
            .notifications
            .contains("cancelled-before-admission")
    );
    f.mode.store(12, Ordering::Relaxed);
    assert!(matches!(
        e.send_delivery_with_cancel(
            "first".into(),
            "Original accepted text".into(),
            vec![],
            &tokio_util::sync::CancellationToken::new()
        )
        .await,
        Err(Error::Storage)
    ));
    assert!(lock(&f.models).is_empty());
    assert!(
        !lock(&f.store)
            .as_ref()
            .unwrap()
            .notifications
            .contains("first")
    );
    f.mode.store(0, Ordering::Relaxed);
    e.send_delivery_with_cancel(
        "first".into(),
        "Changed retry".into(),
        vec![],
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let state = e.snapshot().await;
    assert_eq!(
        state
            .agent("root")
            .unwrap()
            .history
            .iter()
            .filter(|m| m.content == "Original accepted text")
            .count(),
        1
    );
    assert_eq!(lock(&f.models).len(), 1);
    e.cancel_deliveries(&["withdrawn".into()]).await.unwrap();
    assert!(matches!(
        e.send_delivery_with_cancel(
            "withdrawn".into(),
            "Stopped text".into(),
            vec![],
            &tokio_util::sync::CancellationToken::new()
        )
        .await,
        Err(Error::Interrupted)
    ));
    assert_eq!(lock(&f.models).len(), 1);
}
