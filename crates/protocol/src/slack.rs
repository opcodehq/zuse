//! Public Slack settings and attribution. Installation credentials are server-owned.
use serde::{Deserialize, Serialize};

/// Where a user-role message came from when no person typed it into this chat.
/// The message's text parts keep exactly what the agent received; `text` is
/// the short version the transcript shows.
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
    /// A coordinator turn the Zapper engine assembled: its brief, state and
    /// the user's message or thread updates. `text` is the user's own words
    /// when the turn carries them, otherwise a one-line summary of the
    /// updates. Older readers drop this part and show the full prompt.
    Zapper {
        text: String,
        sender: crate::presentation::zapper_message::ZapperSender,
        /// The owner typed this turn: the transcript shows it as their own
        /// message, without the "Sent from Zapper" header.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        typed: bool,
    },
    /// Activity on a pull request this chat owns: a comment, a review, a
    /// check result or a state change, delivered from the GitHub webhook.
    #[serde(rename = "github")]
    GitHub {
        text: String,
        /// `owner/name`.
        repository: String,
        number: u64,
        /// Where the activity is on GitHub (the comment, review or check).
        url: String,
        event: crate::pull_request_links::PullRequestEventKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
}
impl MessageSource {
    /// A coordinator turn that only carries what the owner typed.
    pub fn is_typed(&self) -> bool {
        matches!(self, Self::Zapper { typed: true, .. })
    }
    pub fn text(&self) -> &str {
        match self {
            Self::Slack { text, .. } | Self::Zapper { text, .. } | Self::GitHub { text, .. } => {
                text
            }
        }
    }
    /// The header shown above the short text.
    pub const ZAPPER_LABEL: &str = "Sent from Zapper";
    pub fn label(&self) -> String {
        match self {
            Self::Zapper { .. } => Self::ZAPPER_LABEL.into(),
            Self::GitHub { .. } => self.sender_name(),
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
            Self::Zapper { sender, .. } => sender.name.clone(),
            Self::GitHub {
                repository,
                number,
                author,
                ..
            } => match author {
                Some(author) => format!("{author} on {repository}#{number} · GitHub"),
                None => format!("{repository}#{number} · GitHub"),
            },
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
    pub fn sender(&self) -> crate::presentation::zapper_message::ZapperSender {
        if let Self::Zapper { sender, .. } = self {
            return sender.clone();
        }
        crate::presentation::zapper_message::ZapperSender {
            zapper_id: if self.is_github() { "github" } else { "slack" }.into(),
            chat_id: String::new(),
            name: self.sender_name(),
            avatar: None,
            activity: false,
        }
    }
    pub fn is_slack(&self) -> bool {
        matches!(self, Self::Slack { .. })
    }
    pub fn is_github(&self) -> bool {
        matches!(self, Self::GitHub { .. })
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
/// A Slack sign-in finished in a browser that the Zuse account must confirm
/// before that Slack user can start tasks.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingConnection {
    pub pending_id: String,
    pub team_id: String,
    pub team_name: String,
    pub slack_user_id: String,
    #[serde(default)]
    pub slack_user_name: Option<String>,
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

/// Who a mirrored message is posted as. Slack shows `name` and an avatar the
/// cloud renders from `avatar`, still marked as coming from the Zuse app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<crate::zappers::ZapperAvatar>,
}
impl Persona {
    /// Slack caps display names at 80 characters; avatar ids are short slugs.
    pub fn is_valid(&self) -> bool {
        let name = self.name.trim();
        !name.is_empty()
            && name.chars().count() <= 80
            && !name.chars().any(char::is_control)
            && self
                .avatar
                .as_ref()
                .is_none_or(|a| avatar_id_ok(&a.mascot) && avatar_id_ok(&a.color))
    }
}
pub fn avatar_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A channel a chat can be linked to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub team_id: String,
    pub team_name: String,
    pub channel_id: String,
    pub name: String,
    #[serde(default)]
    pub private: bool,
}

/// The Slack thread a chat mirrors into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLink {
    pub team_id: String,
    pub team_name: String,
    pub channel_id: String,
    pub channel_name: String,
    pub thread_ts: String,
    /// Off keeps the link but stops posting replies and accepting thread replies.
    pub sync: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permalink: Option<String>,
}

/// Cloud request: start a thread for `chat_key` in a channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkRequest {
    pub chat_key: String,
    pub team_id: String,
    pub channel_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<Persona>,
}

/// Cloud request: post one finished reply into a linked thread. `message_id`
/// makes the post idempotent across retries.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostRequest {
    pub chat_key: String,
    pub message_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<Persona>,
}

/// A reply someone wrote in a linked Slack thread, waiting for its chat's engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxMessage {
    pub sequence: i64,
    pub chat_key: String,
    pub event_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

/// Slack's per-message text limit is 40k; keep mirrored replies readable.
pub fn mirror_text(text: &str) -> String {
    let limit = 12_000;
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned() + "\n\n… Open Zuse for the full reply."
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
    fn zapper_turns_carry_their_summary_and_sender() {
        let sender = crate::presentation::zapper_message::ZapperSender {
            zapper_id: "z".into(),
            chat_id: "coord".into(),
            name: "Release".into(),
            avatar: None,
            activity: false,
        };
        let source = MessageSource::Zapper {
            text: "2 thread reports · ZAP-7 changed".into(),
            sender: sender.clone(),
            typed: false,
        };
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json["_tag"], "zapper");
        assert_eq!(json["text"], "2 thread reports · ZAP-7 changed");
        assert!(
            json.get("typed").is_none(),
            "older readers see the same part"
        );
        let back: MessageSource = serde_json::from_value(json).unwrap();
        assert_eq!(back, source);
        assert_eq!(back.label(), "Sent from Zapper");
        assert_eq!(back.sender(), sender);
        assert!(!back.is_slack());
        assert_eq!(back.text(), "2 thread reports · ZAP-7 changed");
        assert!(!back.is_typed());
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
