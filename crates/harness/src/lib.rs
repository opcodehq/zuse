//! Zuse's durable agent runtime. Rust owns execution, recovery, permissions,
//! child scheduling, mailboxes, request budgets, and context compaction.
mod artifact;
pub mod broker;
mod engine;
pub mod evaluation;
pub mod journal;
pub mod native;
#[cfg(unix)]
pub mod native_tools;
pub mod processes;
pub mod prompt;
pub mod state;
pub mod store;
pub mod transport;
use async_trait::async_trait;
pub use engine::{AgentRoute, Engine, Options, SendOutcome, WeakEngine};
use serde::{Deserialize, Serialize};
pub use state::*;
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;

pub type Tools = BTreeMap<String, ToolDefinition>;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub description: String,
    pub parameters: serde_json::Value,
    pub category: ToolCategory,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolCategory {
    Read,
    Edit,
    Execute,
    Network,
    Delegate,
    Other,
    ExitPlan,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    #[default]
    Default,
    Plan,
    AcceptEdits,
}
#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: String,
    pub call_id: String,
    pub cancel: CancellationToken,
    pub permission_mode: PermissionMode,
    pub read_only: bool,
}
#[derive(Clone)]
pub struct ModelInput {
    pub root_id: String,
    pub agent_id: String,
    pub model: String,
    pub reasoning: Option<String>,
    pub messages: Arc<Vec<Message>>,
    pub tools: Arc<Tools>,
    pub purpose: RequestPurpose,
    /// Zero for the first broker attempt; includes retries and account fallback.
    pub retry_index: u32,
    pub cancel: CancellationToken,
    pub text: Arc<dyn Fn(String) + Send + Sync>,
    pub budget: Arc<dyn RequestBudget>,
    pub report: Arc<dyn Fn(transport::Attempt) + Send + Sync>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RequestPurpose {
    #[default]
    Agent,
    Summary,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub connection_id: String,
    pub first_token_ms: Option<f64>,
    #[serde(default)]
    pub uncached_input_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub purpose: RequestPurpose,
    #[serde(default)]
    pub billing_route: Option<String>,
    #[serde(default)]
    pub prefix_fingerprint: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOutput {
    pub messages: Vec<Message>,
    pub calls: Vec<Call>,
    pub text: String,
    #[serde(flatten)]
    pub usage: Usage,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Event {
    UserMessage {
        agent_id: String,
        text: String,
    },
    ModelCompleted {
        agent_id: String,
        item_id: String,
    },
    /// Mail is part of durable history at a model boundary. Replayed delivery
    /// IDs may repeat this acknowledgement without replaying their content.
    MailConsumed {
        agent_id: String,
        delivery_id: String,
    },
    Text {
        agent_id: String,
        item_id: String,
        text: String,
    },
    ToolStart {
        agent_id: String,
        call: Call,
    },
    ToolResult {
        agent_id: String,
        call_id: String,
        output: ToolOutput,
    },
    Usage {
        agent_id: String,
        usage: Usage,
    },
    RequestAttempt {
        agent_id: String,
        attempt: Box<transport::Attempt>,
    },
    Context {
        agent_id: String,
        used_tokens: u64,
        window_tokens: u64,
    },
    Compaction {
        agent_id: String,
        item_id: String,
        started_at: u64,
        status: String,
        covered: usize,
        before: u64,
        after: Option<u64>,
    },
    Paused {
        agent_id: String,
        reason: String,
    },
    Completed {
        agent_id: String,
        summary: String,
    },
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Paused(String),
    #[error("Interrupted")]
    Interrupted,
    #[error("Invalid harness checkpoint. Preserve the journal for inspection.")]
    InvalidCheckpoint,
    #[error("The harness journal could not be saved.")]
    Storage,
    #[error("Model request failed ({code}).")]
    Model { code: String, status: Option<u16> },
    #[error("No eligible connection for this model. Reconnect in account settings.")]
    AccountsUnavailable,
    #[error("{0}")]
    Tool(String),
}
impl Error {
    pub fn paused(message: impl Into<String>) -> Self {
        Self::Paused(message.into())
    }
}
#[async_trait]
pub trait RequestBudget: Send + Sync {
    async fn reserve(&self) -> Result<(), Error>;
}
#[async_trait]
pub trait Host: Send + Sync {
    fn tools(&self) -> Tools;
    async fn load(&self) -> Result<Option<Checkpoint>, Error>;
    async fn save(&self, state: &Checkpoint) -> Result<(), Error>;
    async fn instructions(&self) -> Result<String, Error>;
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error>;
    async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error>;
    fn notify(&self, event: Event);
    async fn interrupt_agents(&self, _ids: &[String]) -> Result<(), Error> {
        Ok(())
    }
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }
}
