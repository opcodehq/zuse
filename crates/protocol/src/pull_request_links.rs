//! Pull requests owned by chats.
//!
//! Each pull request has at most one owning chat per Zuse account. Only the
//! owner hears about activity on it (comments, reviews, check results, state
//! changes), delivered from the GitHub App webhook. Ownership moves to the
//! chat that last opened or pushed to the pull request, or that a person
//! handed it to. A chat may own any number of pull requests.
use serde::{Deserialize, Serialize};

/// How a chat came to own a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimSource {
    /// The chat opened the pull request.
    Created,
    /// The chat pushed to the pull request's head branch.
    Pushed,
    /// A person handed the pull request to the chat.
    Manual,
    /// The pull request is on the chat's own branch and had no owner.
    Branch,
}
impl ClaimSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Pushed => "pushed",
            Self::Manual => "manual",
            Self::Branch => "branch",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "created" => Self::Created,
            "pushed" => Self::Pushed,
            "manual" => Self::Manual,
            "branch" => Self::Branch,
            _ => return None,
        })
    }
    /// A branch claim only takes a pull request nobody owns; every other
    /// claim moves ownership to the claiming chat.
    pub fn takes_over(self) -> bool {
        !matches!(self, Self::Branch)
    }
}

/// What happened on a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PullRequestEventKind {
    /// A conversation comment.
    Comment,
    /// A submitted review (approve, request changes, or comment).
    Review,
    /// An inline comment on the diff.
    ReviewComment,
    /// A check run finished with a failing conclusion.
    ChecksFailed,
    /// Checks passed on a new commit after an earlier failure.
    ChecksPassed,
    Merged,
    Closed,
    Reopened,
}
impl PullRequestEventKind {
    pub const ALL: [Self; 8] = [
        Self::Comment,
        Self::Review,
        Self::ReviewComment,
        Self::ChecksFailed,
        Self::ChecksPassed,
        Self::Merged,
        Self::Closed,
        Self::Reopened,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Review => "review",
            Self::ReviewComment => "review-comment",
            Self::ChecksFailed => "checks-failed",
            Self::ChecksPassed => "checks-passed",
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::Reopened => "reopened",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
    /// Feedback from people: these are combined when several arrive together.
    pub fn is_feedback(self) -> bool {
        matches!(self, Self::Comment | Self::Review | Self::ReviewComment)
    }
    /// Conversation-ending changes, which only wake a recently used chat.
    pub fn is_state_change(self) -> bool {
        matches!(self, Self::Merged | Self::Closed | Self::Reopened)
    }
}

/// What a chat does with one kind of event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventAction {
    /// Run a turn now.
    Wake,
    /// Add it to the chat without running a turn; the agent sees it next time.
    Context,
    /// Drop it.
    Ignore,
}

/// Per-pull-request event settings. Missing entries use [`Self::default_for`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPolicy {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub actions: std::collections::BTreeMap<PullRequestEventKind, EventAction>,
    /// Comments and reviews written by bots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bots: Option<EventAction>,
}
impl EventPolicy {
    pub fn default_for(kind: PullRequestEventKind) -> EventAction {
        match kind {
            PullRequestEventKind::Merged
            | PullRequestEventKind::Closed
            | PullRequestEventKind::Reopened => EventAction::Context,
            _ => EventAction::Wake,
        }
    }
    /// The action for `kind`, written by a bot when `bot` is set.
    pub fn action(&self, kind: PullRequestEventKind, bot: bool) -> EventAction {
        if bot && kind.is_feedback() {
            return self.bots.unwrap_or(EventAction::Ignore);
        }
        self.actions
            .get(&kind)
            .copied()
            .unwrap_or_else(|| Self::default_for(kind))
    }
    pub fn is_valid(&self) -> bool {
        self.actions.len() <= PullRequestEventKind::ALL.len()
    }
}

/// A pull request a chat owns, as the cloud last saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedPullRequest {
    /// `chat:{id}`.
    pub chat_key: String,
    /// `owner/name`.
    pub repository: String,
    pub number: u64,
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    pub claimed_by: ClaimSource,
    pub claimed_at: i64,
    /// The last check result delivered: `failed` or `passed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<String>,
    #[serde(default)]
    pub policy: EventPolicy,
}
impl OwnedPullRequest {
    pub fn chat_id(&self) -> Option<&str> {
        self.chat_key.strip_prefix("chat:")
    }
}

/// Claim a pull request for a chat: by number, or the open pull request
/// whose head is `branch`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimRequest {
    pub chat_key: String,
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub source: ClaimSource,
}

/// One pull request event waiting for the engine that hosts its chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEvent {
    pub sequence: i64,
    pub chat_key: String,
    /// Stable per event; delivery is idempotent on it.
    pub event_id: String,
    pub repository: String,
    pub number: u64,
    pub kind: PullRequestEventKind,
    pub action: EventAction,
    /// Where the activity is on GitHub.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The comment or review body, or the failed check's name and summary.
    #[serde(default)]
    pub body: String,
    /// For inline review comments: `path:line`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub created_at: i64,
}

/// Account feed kind the cloud sends when a pull request owned in this
/// account changes owner or has new inbox events.
pub const FEED_KIND: &str = "pull-request";

/// `https://github.com/{owner}/{name}/pull/{number}` → (`owner/name`, number).
pub fn parse_url(url: &str) -> Option<(String, u64)> {
    let rest = url.trim().strip_prefix("https://github.com/")?;
    let mut parts = rest.trim_end_matches('/').split('/');
    let (owner, name, kind, number) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if kind != "pull" || !valid_component(owner) || !valid_component(name) {
        return None;
    }
    // Trailing `/files`, `/commits` and similar views name the same pull request.
    let number: u64 = number.parse().ok().filter(|n| *n > 0)?;
    Some((format!("{owner}/{name}"), number))
}

/// `owner/name` with GitHub's allowed characters.
pub fn valid_repository(repository: &str) -> bool {
    repository
        .split_once('/')
        .is_some_and(|(owner, name)| valid_component(owner) && valid_component(name))
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// `chat:{id}` keys, bounded like the Slack link keys.
pub fn valid_chat_key(key: &str) -> bool {
    key.strip_prefix("chat:").is_some_and(|id| {
        !id.is_empty() && id.len() <= 200 && !id.chars().any(|c| c.is_control() || c.is_whitespace())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_urls_parse() {
        assert_eq!(
            parse_url("https://github.com/acme/app/pull/12"),
            Some(("acme/app".into(), 12))
        );
        assert_eq!(
            parse_url("https://github.com/acme/app/pull/12/files"),
            Some(("acme/app".into(), 12))
        );
        assert_eq!(parse_url("https://github.com/acme/app/issues/12"), None);
        assert_eq!(parse_url("https://github.com/acme/app/pull/0"), None);
        assert_eq!(parse_url("https://gitlab.com/acme/app/pull/1"), None);
        assert_eq!(parse_url("https://github.com/../app/pull/1"), None);
    }

    #[test]
    fn default_policy_wakes_on_feedback_and_checks_and_ignores_bots() {
        let policy = EventPolicy::default();
        assert_eq!(
            policy.action(PullRequestEventKind::Comment, false),
            EventAction::Wake
        );
        assert_eq!(
            policy.action(PullRequestEventKind::Comment, true),
            EventAction::Ignore
        );
        assert_eq!(
            policy.action(PullRequestEventKind::ChecksFailed, false),
            EventAction::Wake
        );
        // Bots only matter for feedback; a bot-run check still counts.
        assert_eq!(
            policy.action(PullRequestEventKind::ChecksFailed, true),
            EventAction::Wake
        );
        assert_eq!(
            policy.action(PullRequestEventKind::Merged, false),
            EventAction::Context
        );
    }

    #[test]
    fn policy_overrides_round_trip() {
        let mut policy = EventPolicy::default();
        policy
            .actions
            .insert(PullRequestEventKind::Merged, EventAction::Wake);
        policy.bots = Some(EventAction::Context);
        let json = serde_json::to_value(&policy).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"actions":{"merged":"wake"},"bots":"context"})
        );
        let back: EventPolicy = serde_json::from_value(json).unwrap();
        assert_eq!(back, policy);
        assert_eq!(
            back.action(PullRequestEventKind::Review, true),
            EventAction::Context
        );
    }

    #[test]
    fn chat_keys_are_bounded() {
        assert!(valid_chat_key("chat:abc-123"));
        assert!(!valid_chat_key("chat:"));
        assert!(!valid_chat_key("abc"));
        assert!(!valid_chat_key(&format!("chat:{}", "x".repeat(201))));
        assert!(!valid_chat_key("chat:a b"));
    }

    #[test]
    fn github_message_source_round_trips() {
        let source = crate::slack::MessageSource::GitHub {
            text: "Please rename this".into(),
            repository: "acme/app".into(),
            number: 7,
            url: "https://github.com/acme/app/pull/7#issuecomment-1".into(),
            event: PullRequestEventKind::Comment,
            author: Some("ada".into()),
        };
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json["_tag"], "github");
        assert_eq!(json["event"], "comment");
        let back: crate::slack::MessageSource = serde_json::from_value(json).unwrap();
        assert_eq!(back, source);
        assert_eq!(source.sender_name(), "ada on acme/app#7 · GitHub");
        assert!(source.is_github() && !source.is_slack());
    }
}
