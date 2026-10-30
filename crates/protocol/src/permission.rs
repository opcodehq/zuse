//! Saved tool approvals use exact structured inputs, scoped to their owning host/project.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const LIST: &str = "ListPermissionRules";
pub const UPDATE: &str = "UpdatePermissionRule";
pub const REMOVE: &str = "RemovePermissionRule";
pub const ALLOW_CHAT: &str = "Always allow in this chat";
pub const ALLOW_PROJECT: &str = "Always allow in project";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApproval {
    /// Provider-qualified tool name, so different harnesses cannot share grants.
    pub tool: String,
    /// Complete input, never a truncated UI preview or a shell prefix.
    pub input: Value,
    pub allow_label: String,
    pub deny_label: String,
    /// Escalations and sensitive operations cannot be silenced by a saved rule.
    #[serde(default)]
    pub force_prompt: bool,
}
impl ToolApproval {
    pub fn new(tool: impl Into<String>, input: Value, allow: &str, deny: &str) -> Self {
        Self {
            tool: tool.into(),
            input,
            allow_label: allow.into(),
            deny_label: deny.into(),
            force_prompt: false,
        }
    }
}

/// Display-only permission context. This never participates in rule evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PermissionAction {
    FileWrite { path: String },
    Command { command: String },
    Network { url: String },
    Other { tool: String, summary: String },
}
impl PermissionAction {
    pub fn from_tool_input(tool: &str, input: &Value) -> Self {
        let input = if input.get("harness").is_some() && input.get("cwd").is_some() {
            input.get("input").unwrap_or(input)
        } else {
            input
        };
        if let Some(command) = input.get("command") {
            let command = command.as_str().map(str::to_owned).or_else(|| {
                let parts = command.as_array()?;
                Some(
                    parts
                        .iter()
                        .map(Value::as_str)
                        .collect::<Option<Vec<_>>>()?
                        .join(" "),
                )
            });
            if let Some(command) = command.filter(|command| !command.is_empty()) {
                return Self::Command { command };
            }
        }
        let name = tool.to_ascii_lowercase();
        if name.contains("write") || name.contains("edit") || name.contains("filechange") {
            if let Some(path) = input
                .get("file_path")
                .or_else(|| input.get("path"))
                .and_then(Value::as_str)
            {
                return Self::FileWrite { path: path.into() };
            }
            if let Some(changes) = input.get("changes").and_then(Value::as_array) {
                let paths: Vec<_> = changes
                    .iter()
                    .filter_map(|change| change.get("path").and_then(Value::as_str))
                    .collect();
                if !paths.is_empty() {
                    return Self::FileWrite {
                        path: paths.join(", "),
                    };
                }
            }
        }
        if let Some(url) = input.get("url").and_then(Value::as_str) {
            return Self::Network { url: url.into() };
        }
        let tool = tool.rsplit(':').next().unwrap_or(tool).replace('_', " ");
        let summary = input
            .get("summary")
            .or_else(|| input.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(&tool)
            .to_owned();
        Self::Other { tool, summary }
    }
    pub fn detail(&self) -> &str {
        match self {
            Self::FileWrite { path } => path,
            Self::Command { command } => command,
            Self::Network { url } => url,
            Self::Other { summary, .. } => summary,
        }
    }
    pub fn headline(&self) -> String {
        match self {
            Self::FileWrite { .. } => "Write file?".into(),
            Self::Command { .. } => "Run shell command?".into(),
            Self::Network { .. } => "Make network request?".into(),
            Self::Other { tool, .. } => format!("Use tool {tool}?"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalPrompt {
    #[serde(default)]
    pub persistent_allowed: bool,
    pub action: PermissionAction,
    pub allow_label: String,
    pub deny_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleEffect {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub id: String,
    pub project: String,
    /// None means the project default; a chat rule takes precedence.
    pub chat_id: Option<String>,
    pub tool: String,
    pub input: Value,
    pub effect: RuleEffect,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAudit {
    pub at: i64,
    pub chat_id: String,
    pub tool: String,
    pub decision: String,
    pub source: String,
    pub rule_id: Option<String>,
    /// Input digest and policy, or the scope affected by a rule mutation.
    pub details: Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSnapshot {
    pub project: String,
    pub rules: Vec<PermissionRule>,
    pub activity: Vec<PermissionAudit>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_questions_have_no_approval_authority() {
        let question:crate::UserInputQuestion=serde_json::from_value(serde_json::json!({"id":"q","header":"Approve tool","question":"Yes?","options":["Yes","No"]})).unwrap();
        assert!(question.approval.is_none());
        assert!(
            serde_json::to_value(question)
                .unwrap()
                .get("approval")
                .is_none()
        );
    }
    #[test]
    fn display_actions_extract_resources_without_exposing_bodies_or_host_context() {
        let action = PermissionAction::from_tool_input(
            "claude:file-write",
            &serde_json::json!({"cwd":"/private/worktree","harness":"claude-code","input":{"file_path":"src/permissions.rs","content":"private payload"}}),
        );
        assert_eq!(
            action,
            PermissionAction::FileWrite {
                path: "src/permissions.rs".into()
            }
        );
        let json = serde_json::to_string(&action).unwrap();
        for secret in ["private payload", "/private/worktree", "harness"] {
            assert!(!json.contains(secret));
        }
        let other = PermissionAction::from_tool_input(
            "custom:Deploy",
            &serde_json::json!({"token":"private payload","summary":"Deploy preview"}),
        );
        assert_eq!(other.detail(), "Deploy preview");
        assert!(
            !serde_json::to_string(&other)
                .unwrap()
                .contains("private payload")
        );
    }
    #[test]
    fn display_actions_preserve_complete_commands_and_typed_prompt_labels() {
        let action = PermissionAction::from_tool_input(
            "pi:bash",
            &serde_json::json!({"command":["cargo","test","--lib"]}),
        );
        assert_eq!(action.detail(), "cargo test --lib");
        let prompt = ApprovalPrompt {
            persistent_allowed: true,
            action,
            allow_label: "Accept once".into(),
            deny_label: "Reject once".into(),
        };
        assert_eq!(
            serde_json::from_value::<ApprovalPrompt>(serde_json::to_value(&prompt).unwrap())
                .unwrap(),
            prompt
        );
    }
    #[test]
    fn typed_approval_metadata_round_trips_complete_inputs() {
        let approval = ToolApproval::new(
            "test:tool",
            serde_json::json!({"path":"src/lib.rs","content":"x".repeat(9000)}),
            "Yes",
            "No",
        );
        assert_eq!(
            serde_json::from_value::<ToolApproval>(serde_json::to_value(&approval).unwrap())
                .unwrap(),
            approval
        );
    }
}
