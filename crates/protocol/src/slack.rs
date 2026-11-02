//! Public Slack settings and attribution. Installation credentials are server-owned.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag", rename_all = "lowercase")]
pub enum MessageSource {
    Slack {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<String>,
    },
}
impl MessageSource {
    pub fn text(&self) -> &str {
        match self {
            Self::Slack { text, .. } => text,
        }
    }
    pub fn label(&self) -> String {
        match self {
            Self::Slack {
                author, channel, ..
            } => match (author, channel) {
                (Some(author), Some(channel)) => format!("{author} in #{channel} · Slack"),
                (Some(author), None) => format!("{author} · Slack"),
                (None, Some(channel)) => format!("From #{channel} · Slack"),
                _ => "From Slack".into(),
            },
        }
    }
    /// Who sent it, for a "Sent by …" header: "Ada in #eng · Slack", or "Slack".
    pub fn sender_name(&self) -> String {
        match self {
            Self::Slack {
                author, channel, ..
            } => match (author, channel) {
                (Some(author), Some(channel)) => format!("{author} in #{channel} · Slack"),
                (Some(author), None) => format!("{author} · Slack"),
                (None, Some(channel)) => format!("#{channel} · Slack"),
                _ => "Slack".into(),
            },
        }
    }
    /// The same attribution header Zapper messages use, so every message sent
    /// on someone's behalf reads alike.
    pub fn sender(&self) -> crate::presentation::zot_message::ZotSender {
        crate::presentation::zot_message::ZotSender {
            zot_id: "slack".into(),
            chat_id: String::new(),
            name: self.sender_name(),
            avatar: None,
            activity: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub project_id: String,
    pub agent: String,
    pub model: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub team_id: String,
    pub team_name: String,
    pub slack_user_id: String,
    pub project_id: String,
    pub agent: String,
    pub model: String,
}

/// Runtime-owned completion for a specific Slack request, carried in encrypted projections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnResult {
    pub completed_at: i64,
    pub text: String,
    pub status: crate::DoneStatus,
}

/// Keep completion receipts bounded in bytes without splitting UTF-8 characters.
pub fn result_text(text: &str) -> String {
    if text.len() <= 3400 {
        return text.to_owned();
    }
    let mut end = 3400;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned() + "\n\n… Open Zuse for the full result."
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slack_optional_names_have_a_plain_fallback() {
        let source: MessageSource =
            serde_json::from_value(serde_json::json!({"_tag":"slack","text":"Fix it"})).unwrap();
        assert_eq!(source.label(), "From Slack");
        assert_eq!(source.text(), "Fix it");
        assert_eq!(
            serde_json::to_value(source).unwrap(),
            serde_json::json!({"_tag":"slack","text":"Fix it"})
        );
    }
    #[test]
    fn slack_unicode_results_are_bounded_without_cutting_characters() {
        let text = result_text(&"🦀".repeat(1000));
        assert!(text.len() < 3500);
        assert!(text.starts_with("🦀"));
        assert!(text.ends_with("Open Zuse for the full result."));
    }
}

/// A malformed settings reply must never reset the user's selected workspace.
pub fn settings_scope_matches(data: &serde_json::Value, expected: &str) -> bool {
    data["workspaceId"].as_str() == Some(expected)
        && data["executionAvailable"].is_boolean()
        && data["workspaces"].as_array().is_some_and(|choices| {
            choices.iter().all(|choice| {
                choice["id"].as_str().is_some_and(|id| !id.is_empty())
                    && choice["name"]
                        .as_str()
                        .is_some_and(|name| !name.trim().is_empty())
            }) && choices.iter().any(|choice| choice["id"] == "personal")
                && choices.iter().any(|choice| choice["id"] == expected)
        })
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    #[test]
    fn slack_settings_rejects_missing_or_changed_workspace_identity() {
        let valid = serde_json::json!({"workspaceId":"organization:team","executionAvailable":false,"workspaces":[{"id":"personal","name":"Personal"},{"id":"organization:team","name":"Engineering"}]});
        assert!(settings_scope_matches(&valid, "organization:team"));
        assert!(!settings_scope_matches(&valid, "personal"));
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("workspaceId");
        assert!(!settings_scope_matches(&missing, "organization:team"));
        let mut unnamed = valid;
        unnamed["workspaces"][1]
            .as_object_mut()
            .unwrap()
            .remove("name");
        assert!(!settings_scope_matches(&unnamed, "organization:team"));
    }
}
