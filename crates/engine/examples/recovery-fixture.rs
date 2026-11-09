//! Seed an isolated local profile with a real uncertain tool receipt for UI QA.
//! Usage: cargo run -p zuse-engine --example recovery-fixture -- .context/recovery-profile
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Notify;
use zuse_engine::{EngineCore, EngineProfile, HarnessRegistry};
use zuse_harness::{
    Call, Checkpoint, Engine, Error, Event, Host, Message, ModelInput, ModelOutput, Options, Role,
    ToolCategory, ToolContext, ToolDefinition, ToolOutput, Tools, Usage,
};
use zuse_protocol::{ChatConfig, HarnessId};

struct Fixture {
    checkout: PathBuf,
    started: Notify,
}
#[async_trait]
impl Host for Fixture {
    fn tools(&self) -> Tools {
        [(
            "write_file".into(),
            ToolDefinition {
                description: "Write the recovery evidence fixture".into(),
                parameters: json!({"type":"object"}),
                category: ToolCategory::Edit,
            },
        )]
        .into()
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        unreachable!("native journal storage")
    }
    async fn save(&self, _: &Checkpoint) -> Result<(), Error> {
        unreachable!("native journal storage")
    }
    async fn instructions(&self) -> Result<String, Error> {
        Ok("Recovery UI fixture".into())
    }
    async fn model(&self, _: ModelInput) -> Result<ModelOutput, Error> {
        let call = Call {
            id: "fixture-write".into(),
            name: "write_file".into(),
            input: json!({"path":"recovery-evidence.txt","content":"The fixture write completed. Inspect this file before recording the outcome.\n","expected_hash":null}),
        };
        Ok(ModelOutput {
            messages: vec![Message {
                role: Role::Assistant,
                content: json!([{"type":"tool-call","toolCallId":call.id,"toolName":call.name,"input":call.input}]).into(),
                metadata: Default::default(),
            }],
            calls: vec![call],
            text: String::new(),
            usage: Usage::default(),
        })
    }
    async fn execute(&self, _: &str, _: Value, _: ToolContext) -> Result<ToolOutput, Error> {
        tokio::fs::write(
            self.checkout.join("recovery-evidence.txt"),
            "The fixture write completed. Inspect this file before recording the outcome.\n",
        )
        .await
        .map_err(|_| Error::Storage)?;
        self.started.notify_one();
        futures::future::pending().await
    }
    fn notify(&self, _: Event) {}
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Provide an isolated fixture profile directory")?,
    );
    // Protect existing profiles: this fixture owns a new directory only.
    if directory.exists() {
        return Err(
            "Choose a new profile directory; the fixture never replaces existing data.".into(),
        );
    }
    std::fs::create_dir_all(&directory)?;
    let directory = std::fs::canonicalize(directory)?;
    let checkout = directory.join("checkout");
    std::fs::create_dir(&checkout)?;
    let core = EngineCore::assemble_with_profile(
        EngineProfile::local(&directory)?,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Zuse,
        None,
    )?;
    let id = format!("zuse:{}", uuid::Uuid::new_v4());
    let config = ChatConfig {
        plan: false,
        harness: HarnessId::Zuse,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: zuse_protocol::SandboxLevel::WorkspaceWrite,
    };
    core.workspace.create_chat(
        "recovery-ui-fixture",
        None,
        Some(&core.device_id),
        Some(config),
        Some(checkout.to_string_lossy().into_owned()),
    )?;
    core.workspace
        .rename_chat("recovery-ui-fixture", "Review interrupted file write")?;
    core.workspace.set_chat_harness_session(
        "recovery-ui-fixture",
        &id,
        &checkout.to_string_lossy(),
    );
    let doc = core.doc_host.open("recovery-ui-fixture")?;
    for (message_id,role,parts,status) in [
        ("fixture-user",zuse_doc::MessageRole::User,vec![zuse_doc::MessagePart::Text {id:"fixture-request".into(),text:"Create the recovery evidence file".into()}],None),
        ("fixture-interrupted",zuse_doc::MessageRole::Assistant,vec![zuse_doc::MessagePart::Error {id:"fixture-uncertain".into(),message:"The file tool started, then its connection closed before the outcome was saved. Inspect recovery before continuing.".into()}],Some(zuse_doc::MessageStatus::Aborted)),
    ] {
        doc.doc().push_message(&zuse_doc::SessionMessageEntry {id:message_id.into(),role,parts,created_at:chrono::Utc::now().timestamp_millis(),device_id:core.device_id.clone(),status,continuation_of:None,duration_ms:None})?;
    }
    let fixture = Arc::new(Fixture {
        checkout: checkout.clone(),
        started: Notify::new(),
    });
    let engine =
        Engine::open_native(&directory, &id, Options::new("fixture"), fixture.clone()).await?;
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .send("Create the recovery evidence file".into(), vec![])
                .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fixture.started.notified(),
    )
    .await?;
    engine.interrupt(None).await?;
    running.await??;
    engine.close().await?;
    let store = zuse_harness::native::RootStore::acquire(&directory, &id)?;
    std::fs::write(
        store.directory().join("checkout.json"),
        serde_json::to_vec(&checkout)?,
    )?;
    drop(store);
    core.shutdown().await;
    println!("Fixture profile: {}", directory.display());
    println!(
        "Evidence: {}",
        checkout.join("recovery-evidence.txt").display()
    );
    Ok(())
}
