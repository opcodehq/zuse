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
