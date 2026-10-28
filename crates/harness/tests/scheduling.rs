use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use zuse_harness::*;
struct HostFixture {
    saved: Mutex<Option<Checkpoint>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: Mutex<Vec<String>>,
    events: Mutex<Vec<Event>>,
    turns: AtomicUsize,
    hold: bool,
    started: tokio::sync::Notify,
}
#[async_trait]
impl Host for HostFixture {
    fn tools(&self) -> Tools {
        ["read", "question", "edit"]
            .into_iter()
            .map(|name| {
                (
                    name.into(),
                    ToolDefinition {
                        description: name.into(),
                        parameters: json!({"type":"object"}),
                        category: if name == "edit" {
                            ToolCategory::Edit
                        } else {
                            ToolCategory::Read
                        },
                    },
                )
            })
            .collect()
    }
    fn parallel_read(&self, name: &str) -> bool {
        name == "read"
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        Ok(self.saved.lock().unwrap().clone())
    }
    async fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        state.validate()?;
        *self.saved.lock().unwrap() = Some(state.clone());
        Ok(())
    }
    async fn instructions(&self) -> Result<String, Error> {
        Ok("Fixture".into())
    }
    async fn model(&self, _: ModelInput) -> Result<ModelOutput, Error> {
        let calls = if self.turns.fetch_add(1, Ordering::SeqCst) == 0 {
            ["read", "read", "read", "question", "edit", "edit", "read"]
                .into_iter()
                .enumerate()
                .map(|(i, n)| Call {
                    id: i.to_string(),
                    name: n.into(),
                    input: json!({}),
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        let parts = calls
            .iter()
            .map(
                |c| json!({"type":"tool-call","toolCallId":c.id,"toolName":c.name,"input":c.input}),
            )
            .collect::<Vec<_>>();
        Ok(ModelOutput {
            messages: vec![Message {
                role: Role::Assistant,
                content: json!(parts).into(),
                metadata: BTreeMap::new().into(),
            }],
            calls,
            text: "done".into(),
            usage: Usage::default(),
        })
    }
    async fn execute(
        &self,
        name: &str,
        _: Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error> {
        let state = self.saved.lock().unwrap().clone().unwrap();
        assert!(matches!(
            state.tools[&format!("root:{}", context.call_id)].status,
            ReceiptStatus::Started
        ));
        self.started.notify_one();
        if self.hold {
            context.cancel.cancelled().await;
            return Err(Error::Interrupted);
        }
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        if name != "read" {
            assert_eq!(active, 1, "observable actions must stay ordered");
        }
        tokio::time::sleep(std::time::Duration::from_millis(
            if context.call_id == "0" { 40 } else { 5 },
        ))
        .await;
        self.calls.lock().unwrap().push(context.call_id.clone());
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(context.call_id.into())
    }
    fn notify(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
}
#[tokio::test]
async fn independent_reads_overlap_with_ordered_receipts_and_mutation_barriers() {
    let host = Arc::new(HostFixture {
        saved: Mutex::new(None),
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
        events: Mutex::new(vec![]),
        turns: AtomicUsize::new(0),
        hold: false,
        started: tokio::sync::Notify::new(),
    });
    let mut options = Options::new("fixture");
    options.max_parallel_reads = 2;
    let engine = Engine::open("root", options, host.clone()).await.unwrap();
    engine.send("do the work".into(), vec![]).await.unwrap();
    assert_eq!(host.peak.load(Ordering::SeqCst), 2);
    let calls = host.calls.lock().unwrap().clone();
    assert_eq!(&calls[..2], &["1", "0"]);
    let results = host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| {
            if let Event::ToolResult { call_id, .. } = e {
                Some(call_id.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(results, ["0", "1", "2", "3", "4", "5", "6"]);
    let saved = host.saved.lock().unwrap().clone().unwrap();
    assert_eq!(saved.tools.len(), 7);
    assert!(
        saved
            .tools
            .values()
            .all(|r| matches!(r.status, ReceiptStatus::Completed))
    );
    assert!(saved.agents[0].pending_calls.is_empty());
    engine.close().await.unwrap();
}

#[tokio::test]
async fn cancelling_an_admitted_batch_preserves_uncertain_receipts_on_reopen() {
    let host = Arc::new(HostFixture {
        saved: Mutex::new(None),
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
        events: Mutex::new(vec![]),
        turns: AtomicUsize::new(0),
        hold: true,
        started: tokio::sync::Notify::new(),
    });
    let mut options = Options::new("fixture");
    options.max_parallel_reads = 2;
    let engine = Engine::open("root", options.clone(), host.clone())
        .await
        .unwrap();
    let runner = engine.clone();
    let turn = tokio::spawn(async move { runner.send("read".into(), vec![]).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), host.started.notified())
        .await
        .unwrap();
    engine.stop_turn().await.unwrap();
    let _ = turn.await.unwrap();
    engine.close().await.unwrap();
    let state = host.saved.lock().unwrap().clone().unwrap();
    assert_eq!(
        state.tools.len(),
        2,
        "the entire admitted batch starts durably"
    );
    assert!(
        state
            .tools
            .values()
            .all(|r| matches!(r.status, ReceiptStatus::Started))
    );
    let reopened = Engine::open("root", options, host.clone()).await.unwrap();
    reopened.send("continue".into(), vec![]).await.unwrap();
    assert_eq!(
        reopened.snapshot().await.agents[0].status,
        Status::Interrupted
    );
    assert_eq!(
        host.turns.load(Ordering::SeqCst),
        1,
        "no model or tool replay before inspection"
    );
    assert!(host.calls.lock().unwrap().is_empty());
    reopened.close().await.unwrap();
}
