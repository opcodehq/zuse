//! Credential-free startup diagnostic using the REAL harness launch path.
//! The resolver always fails before account/login can be transmitted. No thread,
//! model, auth login or provider request can occur. Safe counters only on stdout.
use futures::StreamExt;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use zuse_providers::codex::auth::{Credential, CredentialResolver};
use zuse_providers::{CancellationToken, CodexHarness, Harness, HarnessError, RunControls};
struct Boundary(AtomicUsize);
#[async_trait::async_trait]
impl CredentialResolver for Boundary {
    async fn resolve(&self, _: bool, _: Option<&str>) -> Result<Credential, HarnessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(HarnessError::Protocol(
            "credential-free-initialize-probe-stop".into(),
        ))
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    anyhow::ensure!(
        matches!(args.len(), 3 | 4),
        "usage: codex_initialize_probe ABSOLUTE_EXECUTABLE ABSOLUTE_CWD [--managed-startup]"
    );
    anyhow::ensure!(
        std::path::Path::new(&args[1]).is_absolute()
            && std::path::Path::new(&args[2]).is_absolute(),
        "absolute paths required"
    );
    anyhow::ensure!(
        args.get(3).is_none_or(|v| v == "--managed-startup"),
        "unknown diagnostic option"
    );
    let boundary = Arc::new(Boundary(AtomicUsize::new(0)));
    let mut harness = CodexHarness::new()
        .with_executable(&args[1])
        .with_credentials(boundary.clone())
        .with_internal_tool_observations(true);
    if args.get(3).is_some_and(|v| v == "--managed-startup") {
        harness = harness.with_managed_startup();
    }
    let request = zuse_protocol::RunRequest {
        prompt: String::new(),
        harness: Some(zuse_protocol::HarnessId::Codex),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: args[2].clone(),
        sandbox: zuse_protocol::SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
    };
    let (_keep, steering) = tokio::sync::mpsc::channel(1);
    let interrupt = CancellationToken::new();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        steering,
        interrupt: interrupt.clone(),
        request_input: Box::new(|_| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
        turn: Default::default(),
    };
    let started = Instant::now();
    let mut stream = harness.start_idle(request, controls).await?;
    let mut count = 0usize;
    let mut category = "no-done";
    let mut observed_done = false;
    let completed = tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(item) = stream.next().await {
            count += 1;
            match item {
                Ok(zuse_protocol::AgentEvent::Done { error, .. }) => {
                    observed_done = true;
                    category = if boundary.0.load(Ordering::SeqCst) > 0 {
                        "stopped-before-login"
                    } else if error.as_deref().is_some_and(|v| {
                        v.contains("initialize") && v.contains("exited before responding")
                    }) {
                        "initialize-child-exited"
                    } else {
                        "other-setup-failure"
                    };
                }
                Err(_) => category = "harness-stream-error",
                _ => {}
            }
        }
    })
    .await;
    if completed.is_err() {
        interrupt.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(8), async {
            while stream.next().await.is_some() {}
        })
        .await;
        category = "initialize-timeout";
    }
    let calls = boundary.0.load(Ordering::SeqCst);
    println!(
        "{}",
        serde_json::json!({"initializeReachedAuthBoundary":calls==1,"resolverCalls":calls,"observedDone":observed_done,"events":count,"category":category,"durationMs":started.elapsed().as_millis(),"authLoginTransmitted":false,"threadStarted":false,"modelCalled":false})
    );
    anyhow::ensure!(
        calls == 1 && observed_done,
        "native initialize did not reach the inert credential boundary"
    );
    Ok(())
}
