//! One isolated process owns history roots and the recording providers.
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
    external_chat::{self as wire, Imported, Row},
};
use zuse_providers::{Harness, HarnessError, RunControls};

struct Recording {
    id: HarnessId,
    requests: Arc<Mutex<Vec<RunRequest>>>,
}
#[async_trait]
impl Harness for Recording {
    fn id(&self) -> HarnessId {
        self.id
    }
    fn display_name(&self) -> &str {
        "Recording import continuation"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                text: "Continued the original provider chat".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: request.resume,
            }),
        ])
        .boxed())
    }
}
fn write(path: &Path, rows: Vec<Value>) -> Vec<u8> {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = rows
        .into_iter()
        .map(|row| row.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    std::fs::write(path, &bytes).unwrap();
    bytes
}
fn registry(requests: Arc<Mutex<Vec<RunRequest>>>) -> Arc<HarnessRegistry> {
    let registry = Arc::new(HarnessRegistry::new());
    for id in [HarnessId::ClaudeCode, HarnessId::Codex] {
        registry.register(Arc::new(Recording {
            id,
            requests: requests.clone(),
        }));
    }
    registry
}
#[tokio::test]
async fn discovery_import_receipts_restart_and_continuation_preserve_provider_history() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("Sample project");
    std::fs::create_dir(&project).unwrap();
    let (claude, codex) = (temp.path().join("claude"), temp.path().join("codex"));
    unsafe {
        std::env::set_var("CLAUDE_CONFIG_DIR", &claude);
        std::env::set_var("CODEX_HOME", &codex);
    }
    let source = claude.join("projects/project/native-session.jsonl");
    let bytes = write(
        &source,
        vec![
            json!({"type":"user","uuid":"user-1","sessionId":"original-claude-session","cwd":project,"message":{"role":"user","content":"Keep the original history"}}),
            json!({"type":"assistant","uuid":"assistant-1","message":{"role":"assistant","model":"fixture","content":[{"type":"text","text":"Before the tool"},{"type":"tool_use","id":"read-1","name":"Read","input":{"file_path":"README.md"}}]}}),
            json!({"type":"user","uuid":"tool-result","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"read-1","content":"Original file bytes"}]}}),
            json!({"type":"assistant","uuid":"assistant-2","message":{"role":"assistant","content":[{"type":"text","text":"After the tool"}]}}),
        ],
    );
    let codex_source = codex.join("archived_sessions/rollout-native.jsonl");
    write(
        &codex_source,
        vec![
            json!({"type":"session_meta","payload":{"id":"original-codex-thread","cwd":project}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Original Codex question"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Original Codex answer"}]}}),
        ],
    );
    write(
        &codex.join("session_index.jsonl"),
        vec![json!({"id":"original-codex-thread","thread_name":"Archived provider conversation"})],
    );
    let requests = Arc::new(Mutex::new(vec![]));
    let profile = temp.path().join("profile");
    let core = EngineCore::assemble(
        &profile,
        registry(requests.clone()),
        HarnessId::ClaudeCode,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    let list: Vec<Row> = client.call_as(wire::LIST, json!({})).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(Row::importable));
    assert!(
        list.iter()
            .any(|row| row.title == "Archived provider conversation")
    );
    assert!(core.workspace.read_chats().unwrap().is_empty());
    assert!(requests.lock().unwrap().is_empty());
    let row = list
        .iter()
        .find(|row| row.provider == HarnessId::ClaudeCode)
        .unwrap();
    let import = json!({"id":row.id,"requestId":uuid::Uuid::new_v4().to_string()});
    let first: Imported = client.call_as(wire::IMPORT, import.clone()).await.unwrap();
    let replay: Imported = client.call_as(wire::IMPORT, import.clone()).await.unwrap();
    assert_eq!(first.chat.id, replay.chat.id);
    assert_eq!(
        first.chat.harness_session_id.as_deref(),
        Some("original-claude-session")
    );
    assert_eq!(first.chat.cwd.as_deref(), project.to_str());
    assert_eq!(first.project.path, project.to_string_lossy());
    let handle = core.doc_host.open_local(&first.chat.id).unwrap();
    let entries = handle.doc().read_entries().unwrap();
    assert_eq!(entries.len(), 2);
    let history = serde_json::to_string(&entries).unwrap();
    for text in ["Before the tool", "After the tool"] {
        assert!(history.contains(text), "Missing {text}: {history}");
    }
    let output_ref = entries[1]
        .parts
        .iter()
        .find_map(|part| match part {
            zuse_doc::MessagePart::Tool { output_ref, .. } => output_ref.clone(),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        core.doc_host
            .fetch_tool_blob_page(&output_ref, 0, 4096)
            .await
            .unwrap()
            .text,
        "Original file bytes"
    );
    assert!(
        requests.lock().unwrap().is_empty(),
        "Import never sends a provider turn"
    );
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    let mut conflict = import.clone();
    conflict["id"] = json!(
        list.iter()
            .find(|r| r.provider == HarnessId::Codex)
            .unwrap()
            .id
    );
    assert!(client.call(wire::IMPORT, conflict).await.is_err());
    drop(handle);
    drop(client);
    core.shutdown().await;
    drop(core);
    let core = EngineCore::assemble(
        &profile,
        registry(requests.clone()),
        HarnessId::ClaudeCode,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    let reopened: Imported = client.call_as(wire::IMPORT, import.clone()).await.unwrap();
    assert_eq!(reopened.chat.id, first.chat.id);
    let reopened_doc = core.doc_host.open_local(&first.chat.id).unwrap();
    assert_eq!(
        serde_json::to_string(&reopened_doc.doc().read_entries().unwrap()).unwrap(),
        history
    );
    drop(reopened_doc);
    assert!(requests.lock().unwrap().is_empty());
    client.call("QueueCommand",json!({"chatId":first.chat.id,"command":{"kind":"run","messageId":"continued-message","request":{"prompt":"Continue from that answer","cwd":project,"harness":"claude-code","sandbox":"workspace-write"}}})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if requests.lock().unwrap().len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let request = requests.lock().unwrap()[0].clone();
    assert_eq!(request.resume.as_deref(), Some("original-claude-session"));
    assert_eq!(request.cwd, project.to_string_lossy());
    let codex_row = list
        .iter()
        .find(|row| row.provider == HarnessId::Codex)
        .unwrap();
    let codex_import: Imported = client
        .call_as(
            wire::IMPORT,
            json!({"id":codex_row.id,"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .await
        .unwrap();
    assert_eq!(
        codex_import.chat.harness_session_id.as_deref(),
        Some("original-codex-thread")
    );
    assert_eq!(
        codex_import.chat.config.as_ref().unwrap().harness,
        HarnessId::Codex
    );
    let codex_doc = core.doc_host.open_local(&codex_import.chat.id).unwrap();
    assert!(
        serde_json::to_string(&codex_doc.doc().read_entries().unwrap())
            .unwrap()
            .contains("Original Codex answer")
    );
    drop(codex_doc);
    core.workspace.delete_chat(&first.chat.id).unwrap();
    assert!(client.call(wire::IMPORT, import).await.is_err());
    assert!(core.workspace.chat(&first.chat.id).unwrap().is_none());
    assert!(client.call(wire::IMPORT,json!({"id":"not-discovered","requestId":uuid::Uuid::new_v4().to_string(),"sourcePath":"/etc/passwd"})).await.is_err());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    drop(client);
    core.shutdown().await;
}
