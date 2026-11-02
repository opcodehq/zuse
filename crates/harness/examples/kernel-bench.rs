//! Scripted kernel workload: no provider calls, subscription usage or UI work.
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
struct Fixture {
    state: Mutex<Option<Checkpoint>>,
    requests: AtomicUsize,
}
fn output(text: &str, calls: Vec<Call>) -> ModelOutput {
    let mut parts = vec![json!({"type":"text","text":text})];
    parts.extend(
        calls.iter().map(
            |c| json!({"type":"tool-call","toolCallId":c.id,"toolName":c.name,"input":c.input}),
        ),
    );
    ModelOutput {
        messages: vec![
            serde_json::from_value(json!({"role":"assistant","content":parts})).unwrap(),
        ],
        calls,
        text: text.into(),
        usage: Usage::default(),
    }
}
#[async_trait]
impl Host for Fixture {
    fn tools(&self) -> Tools {
        BTreeMap::from([(
            "fixture".into(),
            ToolDefinition {
                description: "Deterministic fixture tool".into(),
                parameters: json!({"type":"object"}),
                category: ToolCategory::Read,
            },
        )])
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        Ok(self.state.lock().unwrap().clone())
    }
    async fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }
    async fn instructions(&self) -> Result<String, Error> {
        Ok("Complete the deterministic fixture.".into())
    }
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        if input.tools.is_empty() {
            self.requests.fetch_add(1, Ordering::SeqCst);
            return Ok(output(
                "Original task and tool receipts retained. Continue with recent work.",
                vec![],
            ));
        }
        let count = self.requests.fetch_add(1, Ordering::SeqCst);
        if count.is_multiple_of(2) {
            Ok(output(
                "Read fixture",
                vec![Call {
                    id: format!("call-{count}"),
                    name: "fixture".into(),
                    input: json!({}),
                }],
            ))
        } else {
            for _ in 0..64 {
                (input.text)("streamed fixture ".into());
            }
            Ok(output("Completed fixture", vec![]))
        }
    }
    async fn execute(&self, _: &str, _: Value, _: ToolContext) -> Result<ToolOutput, Error> {
        Ok("verified fixture result".to_string().into())
    }
    fn notify(&self, _: Event) {}
}
fn argument(name: &str, default: usize) -> usize {
    let args = std::env::args().collect::<Vec<_>>();
    args.windows(2).find(|a| a[0] == name).map_or(default, |a| {
        a[1].parse().expect("numeric benchmark argument")
    })
}
fn disk(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .map(|items| {
            items
                .filter_map(Result::ok)
                .map(|e| {
                    if e.path().is_dir() {
                        disk(&e.path())
                    } else {
                        e.metadata().map_or(0, |m| m.len())
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let roots = argument("--roots", 1);
    let history_mib = argument("--history-mib", 1);
    let iterations = argument("--iterations", 8);
    let image = std::env::args().any(|a| a == "--image");
    let compact = std::env::args().any(|a| a == "--compact");
    assert!((1..=20).contains(&roots) && history_mib <= 10 && iterations <= 100);
    let directory = tempfile::tempdir().unwrap();
    let mut engines = Vec::new();
    let mut hosts = Vec::new();
    let mut options = Options::new("fixture");
    options.subagents = false;
    options.window_tokens = if compact { 32000 } else { 100_000_000 };
    for index in 0..roots {
        let host = Arc::new(Fixture {
            state: Mutex::new(None),
            requests: AtomicUsize::new(0),
        });
        let root = format!("root-{index}");
        let seed = Engine::open(&root, options.clone(), host.clone())
            .await
            .unwrap();
        let mut state = seed.snapshot().await;
        seed.close().await.unwrap();
        state
            .agent_mut(&root)
            .unwrap()
            .history
            .push(Message::text(Role::User, "Original fixture task"));
        if image {
            let data = "a".repeat(history_mib * 1024 * 1024);
            state.agent_mut(&root).unwrap().history.push(serde_json::from_value(json!({"role":"user","content":[{"type":"text","text":"Inspect fixture image"},{"type":"image","mediaType":"image/png","image":data}]})).unwrap());
        } else {
            let line = "Historical fixture context. ";
            state.agent_mut(&root).unwrap().history.push(Message::text(
                Role::User,
                line.repeat((history_mib * 1024 * 1024).div_ceil(line.len())),
            ));
        }
        state
            .agent_mut(&root)
            .unwrap()
            .history
            .push(Message::text(Role::Assistant, "Prior work complete"));
        let storage = native::RootStore::acquire(directory.path(), &root).unwrap();
        storage.save(&state).unwrap();
        drop(storage);
        *host.state.lock().unwrap() = None;
        engines.push(
            Engine::open_native(directory.path(), root, options.clone(), host.clone())
                .await
                .unwrap(),
        );
        hosts.push(host);
    }
    println!("READY");
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let initial_bytes = disk(directory.path());
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        let results = futures::future::join_all(
            engines
                .iter()
                .map(|e| e.send("Run fixture task".into(), vec![])),
        )
        .await;
        for result in results {
            result.unwrap();
        }
    }
    let elapsed = start.elapsed().as_secs_f64() * 1000.;
    let final_bytes = disk(directory.path());
    let close = std::time::Instant::now();
    for engine in &engines {
        engine.close().await.unwrap();
    }
    drop(engines);
    let close_ms = close.elapsed().as_secs_f64() * 1000.;
    let reopen = std::time::Instant::now();
    for index in 0..roots {
        let store = native::RootStore::acquire(directory.path(), &format!("root-{index}")).unwrap();
        let saved = store.load().unwrap().unwrap();
        assert!(
            saved
                .tools
                .values()
                .all(|r| matches!(r.status, ReceiptStatus::Completed))
        );
    }
    let replay_ms = reopen.elapsed().as_secs_f64() * 1000.;
    println!(
        "{}",
        json!({"roots":roots,"history_mib":history_mib,"image":image,"compact":compact,"iterations":iterations,"transport":"scripted","elapsed_ms":elapsed,"replay_ms":replay_ms,"close_ms":close_ms,"turn_close_replay_ms":start.elapsed().as_secs_f64()*1000.,"initial_disk_bytes":initial_bytes,"final_disk_bytes":final_bytes,"disk_growth_bytes":final_bytes.saturating_sub(initial_bytes),"model_requests":hosts.iter().map(|h|h.requests.load(Ordering::SeqCst)).sum::<usize>()})
    );
    drop(hosts);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}
