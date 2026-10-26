use crate::Error;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
/// Cheap immutable snapshots; mutation copies only the affected payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Shared<T>(std::sync::Arc<T>);
impl<T> Shared<T> {
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}
impl<T> From<T> for Shared<T> {
    fn from(value: T) -> Self {
        Self(std::sync::Arc::new(value))
    }
}
impl<T> std::ops::Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Clone> std::ops::DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        std::sync::Arc::make_mut(&mut self.0)
    }
}
impl PartialEq<&str> for Shared<Value> {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_ref() == *other
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Shared<Value>,
    #[serde(flatten)]
    pub metadata: Shared<BTreeMap<String, Value>>,
}
impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: Value::String(text.into()).into(),
            metadata: BTreeMap::new().into(),
        }
    }
    pub fn tool(call: &Call, output: &ToolOutput) -> Self {
        Self {
            role: Role::Tool,
            content: json!([{"type":"tool-result","toolCallId":call.id,"toolName":call.name,"output":output.model_value()}]).into(),
            metadata: BTreeMap::new().into(),
        }
    }
    pub fn has_tool_calls(&self) -> bool {
        self.role == Role::Assistant
            && self
                .content
                .as_array()
                .is_some_and(|p| p.iter().any(|v| v["type"] == "tool-call"))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub media_type: String,
    pub data: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
}
impl Mail {
    pub fn message(&self) -> Message {
        if self.images.is_empty() {
            return Message::text(Role::User, self.text.clone());
        }
        let mut content = vec![json!({"type":"text","text":self.text})];
        content.extend(
            self.images
                .iter()
                .map(|i| json!({"type":"image","image":i.data,"mediaType":i.media_type})),
        );
        Message {
            role: Role::User,
            content: Value::Array(content).into(),
            metadata: BTreeMap::new().into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub input: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolOutput {
    Text(String),
    Content {
        r#type: String,
        value: Vec<ImageContent>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    pub r#type: String,
    pub media_type: String,
    pub data: ImageData,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    pub r#type: String,
    pub data: String,
}
impl ToolOutput {
    pub fn model_value(&self) -> Value {
        match self {
            Self::Text(value) => json!({"type":"text","value":value}),
            other => serde_json::to_value(other).unwrap_or(Value::Null),
        }
    }
    pub fn validate(&self) -> bool {
        match self {
            Self::Text(_) => true,
            Self::Content { r#type, value } => {
                r#type == "content"
                    && value.iter().all(|i| {
                        i.r#type == "file"
                            && matches!(
                                i.media_type.as_str(),
                                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                            )
                            && i.data.r#type == "data"
                    })
            }
        }
    }
}
impl From<String> for ToolOutput {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Idle,
    Running,
    Completed,
    Interrupted,
    Error,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compaction {
    pub covered: usize,
    pub summary: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub parent_item_id: Option<String>,
    pub depth: u8,
    pub read_only: bool,
    pub history: Vec<Message>,
    pub mailbox: Vec<Mail>,
    pub status: Status,
    pub turns: u64,
    pub summary: String,
    pub pending_calls: Vec<Call>,
    pub completed_delivery: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<Compaction>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptStatus {
    Started,
    Completed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub status: ReceiptStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Shared<ToolOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}
/// Only a trusted user control surface may submit this decision; it is never a model tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum RecoveryDecision {
    RetryAfterInspection,
    RecordOutcome { output: ToolOutput },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryReview {
    pub agent_id: String,
    pub call_id: String,
    pub action: String,
    pub at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub version: u8,
    pub root_id: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub agents: Vec<Agent>,
    pub requests: u64,
    #[serde(default)]
    pub notifications: BTreeSet<String>,
    #[serde(default)]
    pub consumed_notifications: BTreeMap<String, String>,
    #[serde(default)]
    pub cancelled_notifications: BTreeSet<String>,
    pub tools: BTreeMap<String, Receipt>,
    #[serde(default)]
    pub reviews: Vec<RecoveryReview>,
}
impl Checkpoint {
    /// Trusted recovery control only. This mutates no runtime and starts no work.
    pub fn resolve_unknown(
        &mut self,
        agent_id: &str,
        call_id: &str,
        decision: RecoveryDecision,
    ) -> Result<(), Error> {
        if !self
            .agent(agent_id)?
            .pending_calls
            .iter()
            .any(|c| c.id == call_id)
        {
            return Err(Error::Tool("Unknown pending call".into()));
        }
        let key = format!("{agent_id}:{call_id}");
        if !self
            .tools
            .get(&key)
            .is_some_and(|r| matches!(r.status, ReceiptStatus::Started))
        {
            return Err(Error::Tool(
                "This call does not need an outcome review.".into(),
            ));
        }
        let action = match decision {
            RecoveryDecision::RetryAfterInspection => {
                self.tools.remove(&key);
                "retryAfterInspection"
            }
            RecoveryDecision::RecordOutcome { output } => {
                if !output.validate() {
                    return Err(Error::Tool("Invalid inspected tool result".into()));
                }
                let receipt = self.tools.get_mut(&key).unwrap();
                receipt.status = ReceiptStatus::Completed;
                receipt.output = Some(output.into());
                "recordOutcome"
            }
        };
        self.reviews.push(RecoveryReview {
            agent_id: agent_id.into(),
            call_id: call_id.into(),
            action: action.into(),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        Ok(())
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != 1 || self.root_id.is_empty() || self.agents.len() > 4096 {
            return Err(Error::InvalidCheckpoint);
        }
        if self.consumed_notifications.iter().any(|(id, agent)| {
            !self.notifications.contains(id) || !self.agents.iter().any(|a| &a.id == agent)
        }) {
            return Err(Error::InvalidCheckpoint);
        }
        if self.cancelled_notifications.iter().any(|id| {
            !self.notifications.contains(id)
                || self.consumed_notifications.contains_key(id)
                || self
                    .agents
                    .iter()
                    .any(|agent| agent.mailbox.iter().any(|mail| &mail.id == id))
        }) {
            return Err(Error::InvalidCheckpoint);
        }
        let by_id: BTreeMap<_, _> = self.agents.iter().map(|a| (a.id.as_str(), a)).collect();
        if by_id.len() != self.agents.len()
            || self.agents.iter().filter(|a| a.parent_id.is_none()).count() != 1
        {
            return Err(Error::InvalidCheckpoint);
        }
        let root = by_id
            .get(self.root_id.as_str())
            .ok_or(Error::InvalidCheckpoint)?;
        if root.depth != 0 || root.parent_id.is_some() {
            return Err(Error::InvalidCheckpoint);
        }
        for a in &self.agents {
            if a.id.is_empty()
                || a.depth > 2
                || a.checkpoint
                    .as_ref()
                    .is_some_and(|c| c.covered > a.history.len())
            {
                return Err(Error::InvalidCheckpoint);
            }
            if let Some(parent) = &a.parent_id {
                if !by_id
                    .get(parent.as_str())
                    .is_some_and(|p| p.depth + 1 == a.depth)
                {
                    return Err(Error::InvalidCheckpoint);
                }
            }
            let mut calls = BTreeSet::new();
            if a.pending_calls
                .iter()
                .any(|c| c.id.is_empty() || c.name.is_empty() || !calls.insert(&c.id))
            {
                return Err(Error::InvalidCheckpoint);
            }
            if a.history
                .iter()
                .any(|m| !m.content.is_string() && !m.content.is_array())
            {
                return Err(Error::InvalidCheckpoint);
            }
        }
        if self.tools.values().any(|r| {
            r.output.as_ref().is_some_and(|v| !v.validate())
                || matches!(r.status, ReceiptStatus::Completed) && r.output.is_none()
        }) {
            return Err(Error::InvalidCheckpoint);
        }
        Ok(())
    }
    pub fn agent(&self, id: &str) -> Result<&Agent, Error> {
        self.agents
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::Tool("Unknown child".into()))
    }
    pub fn agent_mut(&mut self, id: &str) -> Result<&mut Agent, Error> {
        self.agents
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::Tool("Unknown child".into()))
    }
    pub fn descendant(&self, id: &str, parent: &str) -> bool {
        let mut current = id;
        for _ in 0..=2 {
            let Ok(agent) = self.agent(current) else {
                return false;
            };
            let Some(next) = agent.parent_id.as_deref() else {
                return false;
            };
            if next == parent {
                return true;
            }
            current = next;
        }
        false
    }
}

impl From<String> for Shared<ToolOutput> {
    fn from(value: String) -> Self {
        Self::from(ToolOutput::from(value))
    }
}
