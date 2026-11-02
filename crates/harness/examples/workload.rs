//! Deterministic concurrent runtime workload. No network or model credentials.
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use zuse_harness::{store::FileStore, *};

struct Replay {
    store: FileStore,
    input: PathBuf,
    requests: AtomicU64,
    saves: AtomicU64,
    written: AtomicU64,
    tools: AtomicU64,
    chunks: AtomicU64,
}

#[async_trait]
impl Host for Replay {
    fn tools(&self) -> Tools {
        BTreeMap::from([(
            "read_fixture".into(),
            ToolDefinition {
                description: "Read the local workload fixture".into(),
                parameters: json!({"type":"object", "properties":{}}),
                category: ToolCategory::Read,
            },
        )])
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        self.store.load()
    }
    async fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        self.store.save(state)?;
        self.saves.fetch_add(1, Ordering::Relaxed);
        self.written.fetch_add(
            std::fs::metadata(self.store.path())
                .map_err(|_| Error::Storage)?
                .len(),
            Ordering::Relaxed,
        );
        Ok(())
    }
    async fn instructions(&self) -> Result<String, Error> {
        Ok("Read the fixture and report the result.".into())
    }
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        let request = self.requests.fetch_add(1, Ordering::Relaxed);
        let calls = if request.is_multiple_of(2) {
            vec![Call {
                id: format!("read-{request}"),
                name: "read_fixture".into(),
                input: json!({}),
            }]
        } else {
            vec![]
        };
        let mut text = String::new();
        for _ in 0..32 {
            let chunk = "Deterministic replay response.\n";
            (input.text)(chunk.into());
            text.push_str(chunk);
            tokio::task::yield_now().await;
        }
        let mut content = vec![json!({"type":"text", "text":text})];
        for call in &calls {
            content.push(json!({"type":"tool-call", "toolCallId":call.id,
                "toolName":call.name, "input":call.input}));
        }
        Ok(ModelOutput {
            messages: vec![Message {
                role: Role::Assistant,
                content: json!(content).into(),
                metadata: BTreeMap::new().into(),
            }],
            calls,
            text,
            usage: Usage::default(),
        })
    }
    async fn execute(
        &self,
        name: &str,
        _: serde_json::Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error> {
        if name != "read_fixture" || context.cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        let content = tokio::fs::read_to_string(&self.input)
            .await
            .map_err(|_| Error::Storage)?;
        self.tools.fetch_add(1, Ordering::Relaxed);
        Ok(ToolOutput::Text(content))
    }
    fn notify(&self, event: Event) {
        if let Event::Paused { agent_id, reason } = &event {
            eprintln!("Replay paused for {agent_id}: {reason}");
        }
        if matches!(event, Event::Text { .. }) {
            self.chunks.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn history<T: From<Message>>(events: usize) -> Vec<T> {
    let payload = "Workload text: code, paths, Unicode λ, and escaped \"values\".\n".repeat(4);
    (0..events)
        .map(|index| {
            let call = Call {
                id: format!("seed-{index}"),
                name: "read_fixture".into(),
                input: json!({}),
            };
            let message = match index % 4 {
                0 => Message::text(Role::User, format!("Task {index}: {payload}")),
                1 => Message {
                    role: Role::Assistant,
                    content:
                        json!([{"type":"tool-call", "toolCallId":format!("seed-{}", index + 1),
                        "toolName":"read_fixture", "input":{}}])
                        .into(),
                    metadata: BTreeMap::new().into(),
                },
                2 => Message::tool(&call, &ToolOutput::Text(payload.clone())),
                _ => Message::text(Role::Assistant, format!("Result {index}: {payload}")),
            };
            message.into()
        })
        .collect()
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        return Err("Usage: workload THREADS EVENTS_PER_THREAD ROUNDS OUTPUT.json".into());
    }
    let threads: usize = args[0].parse()?;
    let events: usize = args[1].parse()?;
    let rounds: usize = args[2].parse()?;
    if threads == 0 || events == 0 || rounds == 0 {
        return Err("Threads, events and rounds must be positive".into());
    }
    let profile = tempfile::tempdir()?;
    let input = profile.path().join("input.txt");
    std::fs::write(&input, "Local tool fixture. No network access.\n")?;
    let seed = Instant::now();
    let mut sessions = Vec::new();
    let mut initial_bytes = 0;
    for index in 0..threads {
        let host = Arc::new(Replay {
            store: FileStore::new(
                profile
                    .path()
                    .join(format!("thread-{index}/checkpoint.json")),
            ),
            input: input.clone(),
            requests: AtomicU64::new(0),
            saves: AtomicU64::new(0),
            written: AtomicU64::new(0),
            tools: AtomicU64::new(0),
            chunks: AtomicU64::new(0),
        });
        let root_id = format!("thread-{index}");
        let state = Checkpoint {
            version: 1,
            root_id: root_id.clone(),
            model: "local-replay".into(),
            reasoning: None,
            requests: 0,
            notifications: Default::default(),
            consumed_notifications: Default::default(),
            cancelled_notifications: Default::default(),
            tools: Default::default(),
            reviews: vec![],
            agents: vec![Agent {
                id: root_id.clone(),
                name: "root".into(),
                parent_id: None,
                parent_item_id: None,
                depth: 0,
                read_only: false,
                history: history(events),
                mailbox: vec![],
                status: Status::Idle,
                turns: 0,
                summary: String::new(),
                pending_calls: vec![],
                completed_delivery: false,
                checkpoint: None,
            }],
        };
        host.store.save(&state)?;
        initial_bytes += std::fs::metadata(host.store.path())?.len();
        drop(state);
        let mut options = Options::new("local-replay");
        options.window_tokens = 16_000_000;
        options.subagents = false;
        sessions.push((Engine::open(root_id, options, host.clone()).await?, host));
    }
    let seed_ms = seed.elapsed().as_secs_f64() * 1000.;
    // Keep all loaded runtimes resident before measurement, including inactive histories.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let started = Instant::now();
    let mut running = tokio::task::JoinSet::new();
    for (engine, host) in sessions {
        running.spawn(async move {
            let start = Instant::now();
            for round in 0..rounds {
                engine
                    .send(format!("Read the fixture, round {round}"), vec![])
                    .await?;
            }
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            let state = engine.snapshot().await;
            if state.agents[0].history.len() != events + rounds * 4
                || !state.agents[0].pending_calls.is_empty()
                || host.tools.load(Ordering::Relaxed) != rounds as u64
                || host.requests.load(Ordering::Relaxed) != (rounds * 2) as u64
                || host.chunks.load(Ordering::Relaxed) != (rounds * 64) as u64
            {
                return Err(Error::paused(
                    "Replay lost events or executed an unexpected number of tools",
                ));
            }
            drop(state);
            engine.close().await?;
            let recovered = host.store.load()?.ok_or(Error::Storage)?;
            if recovered.agents[0].history.len() != events + rounds * 4 {
                return Err(Error::InvalidCheckpoint);
            }
            Ok::<_, Error>((elapsed, host))
        });
    }
    let mut latencies = vec![];
    let (mut saves, mut written, mut tools, mut chunks) = (0, 0, 0, 0);
    while let Some(result) = running.join_next().await {
        let (latency, host) = result??;
        latencies.push(latency);
        saves += host.saves.load(Ordering::Relaxed);
        written += host.written.load(Ordering::Relaxed);
        tools += host.tools.load(Ordering::Relaxed);
        chunks += host.chunks.load(Ordering::Relaxed);
    }
    let elapsed = started.elapsed().as_secs_f64();
    latencies.sort_by(f64::total_cmp);
    let result = json!({
        "scope":"Harness only: retained histories, concurrent replay, real file reads and durable checkpoints. Not desktop rendering or provider/network latency.",
        "threads":threads, "events_per_thread":events, "rounds":rounds,
        "transport":"direct-local-replay",
        "seed_ms":seed_ms, "initial_checkpoint_bytes":initial_bytes,
        "workload_ms":elapsed * 1000., "turns_per_second":(threads * rounds) as f64 / elapsed,
        "thread_latency_p50_ms":latencies[latencies.len() / 2],
        "thread_latency_p95_ms":latencies[((latencies.len() as f64 * 0.95).ceil() as usize - 1).min(latencies.len() - 1)],
        "checkpoint_saves":saves, "checkpoint_bytes_written":written,
        "tool_executions":tools, "stream_chunks":chunks, "recovery_verified":true,
    });
    std::fs::write(
        &args[3],
        format!("{}\n", serde_json::to_string_pretty(&result)?),
    )?;
    println!("{result}");
    Ok(())
}
