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
    /// The branch can no longer merge cleanly into its base.
    Conflict,
}
impl PullRequestEventKind {
    pub const ALL: [Self; 9] = [
        Self::Comment,
        Self::Review,
        Self::ReviewComment,
        Self::ChecksFailed,
        Self::ChecksPassed,
        Self::Merged,
        Self::Closed,
        Self::Reopened,
        Self::Conflict,
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
            Self::Conflict => "conflict",
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
    /// The cloud computer hosting the chat, for cloud chats; their events are
    /// delivered by the cloud rather than a desktop engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
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
    /// The cloud computer hosting the chat, when it is a cloud chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
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

/// Someone asked Zuse for help (`@zuse`) on a pull request no chat of theirs
/// owns. The engine starts a chat for it in a project with that repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestMention {
    pub sequence: i64,
    pub event_id: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    pub kind: PullRequestEventKind,
    /// Where the mention is on GitHub.
    pub comment_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub created_at: i64,
}

/// One line (or a few) about an event, as the agent reads it.
pub fn describe(event: &InboxEvent) -> String {
    let who = event.author.as_deref().unwrap_or("Someone");
    let body = event.body.trim();
    let mut line = match event.kind {
        PullRequestEventKind::Comment => format!("{who} commented:\n{body}"),
        PullRequestEventKind::Review => format!("{who} reviewed: {body}"),
        PullRequestEventKind::ReviewComment => match &event.location {
            Some(at) => format!("{who} commented on {at}:\n{body}"),
            None => format!("{who} commented on the diff:\n{body}"),
        },
        PullRequestEventKind::ChecksFailed => match &event.author {
            Some(app) => format!("A check failed ({app}): {body}"),
            None => format!("A check failed: {body}"),
        },
        PullRequestEventKind::ChecksPassed => "Checks passed on the latest commit.".into(),
        PullRequestEventKind::Merged => format!("{who} merged it."),
        PullRequestEventKind::Closed => format!("{who} closed it without merging."),
        PullRequestEventKind::Reopened => format!("{who} reopened it."),
        PullRequestEventKind::Conflict if body.is_empty() => {
            "It has merge conflicts with its base branch.".into()
        }
        PullRequestEventKind::Conflict => body.to_owned(),
    };
    if !event.url.is_empty() {
        line.push_str(&format!("\n{}", event.url));
    }
    line
}

/// The full text the agent receives, grouped by pull request.
pub fn activity_text(events: &[InboxEvent]) -> String {
    let mut groups: Vec<((String, u64), Vec<&InboxEvent>)> = Vec::new();
    for event in events {
        let key = (event.repository.clone(), event.number);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(event),
            None => groups.push((key, vec![event])),
        }
    }
    groups
        .into_iter()
        .map(|((repository, number), list)| {
            let items = list
                .iter()
                .map(|e| format!("- {}", describe(e).replace('\n', "\n  ")))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "Activity on pull request {repository}#{number} (https://github.com/{repository}/pull/{number}):\n{items}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The short line a transcript shows for a delivery.
pub fn summary(events: &[InboxEvent]) -> String {
    if let [event] = events {
        let line = describe(event);
        let line = line.lines().take(2).collect::<Vec<_>>().join(" ");
        return line.chars().take(280).collect();
    }
    let mut prs: Vec<(&str, u64)> = events
        .iter()
        .map(|e| (e.repository.as_str(), e.number))
        .collect();
    prs.sort_unstable();
    prs.dedup();
    match (prs.as_slice(), events.first()) {
        ([_], Some(first)) => format!(
            "{} updates on {}#{}",
            events.len(),
            first.repository,
            first.number
        ),
        _ => format!("{} updates on {} pull requests", events.len(), prs.len()),
    }
}

/// What the agent is asked to do after activity that starts a turn.
pub const WAKE_INSTRUCTION: &str = "You own this pull request. Handle what needs a change, push the fix, and reply on GitHub with `gh` when that helps the reviewer.";

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
        !id.is_empty()
            && id.len() <= 200
            && !id.chars().any(|c| c.is_control() || c.is_whitespace())
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

    fn event(kind: PullRequestEventKind, action: EventAction) -> InboxEvent {
        InboxEvent {
            sequence: 1,
            chat_key: "chat:c".into(),
            event_id: format!("{kind:?}"),
            repository: "acme/app".into(),
            number: 7,
            kind,
            action,
            url: "https://github.com/acme/app/pull/7#issuecomment-1".into(),
            author: Some("bob".into()),
            body: "Please rename this".into(),
            location: None,
            created_at: 0,
        }
    }

    #[test]
    fn activity_is_grouped_by_pull_request() {
        let mut failed = event(PullRequestEventKind::ChecksFailed, EventAction::Wake);
        failed.author = Some("GitHub Actions".into());
        failed.body = "test failed".into();
        failed.url = "https://github.com/acme/app/actions/runs/1".into();
        let mut other = event(PullRequestEventKind::Review, EventAction::Wake);
        other.number = 8;
        other.body = "Approved".into();
        let text = activity_text(&[
            event(PullRequestEventKind::Comment, EventAction::Wake),
            failed,
            other,
        ]);
        assert_eq!(
            text,
            "Activity on pull request acme/app#7 (https://github.com/acme/app/pull/7):\n\
             - bob commented:\n  Please rename this\n  https://github.com/acme/app/pull/7#issuecomment-1\n\
             - A check failed (GitHub Actions): test failed\n  https://github.com/acme/app/actions/runs/1\n\n\
             Activity on pull request acme/app#8 (https://github.com/acme/app/pull/8):\n\
             - bob reviewed: Approved\n  https://github.com/acme/app/pull/7#issuecomment-1"
        );
    }

    #[test]
    fn summaries_are_short() {
        let one = [event(PullRequestEventKind::Comment, EventAction::Wake)];
        assert_eq!(summary(&one), "bob commented: Please rename this");
        let two = [
            event(PullRequestEventKind::Comment, EventAction::Wake),
            event(PullRequestEventKind::Review, EventAction::Wake),
        ];
        assert_eq!(summary(&two), "2 updates on acme/app#7");
    }

    #[test]
    fn conflicts_read_plainly() {
        let mut conflict = event(PullRequestEventKind::Conflict, EventAction::Wake);
        conflict.body = String::new();
        conflict.url = String::new();
        assert_eq!(
            describe(&conflict),
            "It has merge conflicts with its base branch."
        );
    }
}
