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
        if self.mode.load(Ordering::Relaxed) == 7 && !state.reviews.is_empty() {
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
async fn checkpoint_graph_corruption_is_rejected_without_resetting_history() {
    let f = Fixture::new(vec![]);
    let e = engine(f.clone()).await;
    let state = e.snapshot().await;
    let directory = tempfile::tempdir().unwrap();
    let store =
        zuse_harness::store::FileStore::new(directory.path().join("session/checkpoint.json"));
    store.save(&state).unwrap();
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
    state.agent_mut("root").unwrap().history.push(Message::text(
        Role::User,
        "Finish the complete request; retain pending work.",
    ));
    for _ in 0..20 {
        state
            .agent_mut("root")
            .unwrap()
            .history
            .push(Message::text(Role::User, "old context ".repeat(100)));
        state
            .agent_mut("root")
            .unwrap()
            .history
            .push(Message::text(Role::Assistant, "prior outcome ".repeat(100)));
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
