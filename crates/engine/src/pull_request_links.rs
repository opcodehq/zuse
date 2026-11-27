//! Pull requests owned by this engine's chats.
//!
//! The cloud records which chat owns each pull request and turns GitHub
//! webhooks for it into inbox events. This engine claims pull requests for
//! its chats (when one opens a pull request or pushes to its branch), and
//! delivers inbox events into the owning chat as soon as the account feed's
//! `pull-request` hint arrives. It reads the inbox on a timer only while the
//! feed is down.
//!
//! Delivery is idempotent: event ids are remembered before they are
//! acknowledged, so a crash in between never delivers an event twice.

use crate::cloud::identity::CloudIdentity as Auth;
use crate::{DocHost, SessionsEngine, WorkspaceHost};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, watch};
use zuse_doc::{SessionCommandPayload, SessionMessageEntry};
use zuse_model::{MessagePart, MessageRole, MessageStatus};
use zuse_protocol::methods;
use zuse_protocol::pull_request_links::{
    ClaimSource, EventAction, EventPolicy, FEED_KIND, InboxEvent, OwnedPullRequest,
    PullRequestEventKind,
};
use zuse_protocol::slack::MessageSource;

/// Comments and reviews that arrive together are delivered as one message
/// once none has arrived for this long.
const FEEDBACK_QUIET: Duration = Duration::from_secs(20);
/// Inbox reads while the account feed is down.
const FALLBACK_EVERY: Duration = Duration::from_secs(60);
/// Merges, closes and reopens only wake a chat someone wrote in this recently.
const STATE_CHANGE_RECENCY_MS: i64 = 3 * 24 * 60 * 60 * 1000;
/// Wakes in a row from pull request activity alone, with nobody writing in
/// the chat, before further activity is only kept as context.
const WAKE_LIMIT: usize = 10;
/// Delivered event ids remembered for idempotency.
const DELIVERED_LIMIT: usize = 2000;

#[derive(Clone)]
pub struct PullRequestLinks {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    state: Mutex<State>,
    workspace: WorkspaceHost,
    docs: DocHost,
    sessions: SessionsEngine,
    pr_watches: crate::pr_watch::PrWatches,
    auth: Mutex<Option<Auth>>,
    updates: watch::Sender<Vec<OwnedPullRequest>>,
    wake: Arc<Notify>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    follow: OnceLock<()>,
    /// When each pending feedback event was first seen here.
    seen: Mutex<HashMap<String, Instant>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(default)]
    owned: Vec<OwnedPullRequest>,
    /// Activity kept for each chat's next turn.
    #[serde(default)]
    context: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    delivered: VecDeque<String>,
}

fn chat_key(chat: &str) -> String {
    format!("chat:{chat}")
}

fn short_id(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())[..32].to_owned()
}

pub fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::CLAIM_PULL_REQUEST
            | methods::RELEASE_PULL_REQUEST
            | methods::SET_PULL_REQUEST_POLICY
    )
}

impl PullRequestLinks {
    pub fn new(
        store_root: &std::path::Path,
        workspace: WorkspaceHost,
        docs: DocHost,
        sessions: SessionsEngine,
        pr_watches: crate::pr_watch::PrWatches,
    ) -> Self {
        let path = store_root.join("pull-request-links.json");
        let state: State = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let (updates, _) = watch::channel(state.owned.clone());
        let links = Self {
            inner: Arc::new(Inner {
                path,
                state: Mutex::new(state),
                workspace,
                docs,
                sessions: sessions.clone(),
                pr_watches,
                auth: Mutex::new(None),
                updates,
                wake: Arc::new(Notify::new()),
                task: Mutex::new(None),
                follow: OnceLock::new(),
                seen: Mutex::new(HashMap::new()),
            }),
        };
        let weak = Arc::downgrade(&links.inner);
        sessions.set_prompt_context(Arc::new(move |chat: &str| {
            let inner = weak.upgrade()?;
            Self { inner }.take_context(chat)
        }));
        links
    }

    /// Act for this account: (re)start the feed reader and the turn follower.
    pub fn attach(&self, auth: Auth) {
        *lock(&self.inner.auth) = Some(auth);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut task = lock(&self.inner.task);
        if let Some(old) = task.take() {
            old.abort();
        }
        *task = Some(runtime.spawn(Self::run(
            Arc::downgrade(&self.inner),
            self.inner.wake.clone(),
        )));
        drop(task);
        if self.inner.follow.set(()).is_ok() {
            runtime.spawn(Self::follow_turns(
                Arc::downgrade(&self.inner),
                self.inner.sessions.watch_sessions(),
            ));
        }
    }

    pub async fn shutdown(&self) {
        let task = lock(&self.inner.task).take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }

    /// Owned pull requests, optionally only `chat`'s, as they change.
    pub fn subscribe(&self) -> watch::Receiver<Vec<OwnedPullRequest>> {
        self.inner.updates.subscribe()
    }

    pub fn owned(&self, chat: Option<&str>) -> Vec<OwnedPullRequest> {
        let state = lock(&self.inner.state);
        state
            .owned
            .iter()
            .filter(|pr| chat.is_none_or(|chat| pr.chat_id() == Some(chat)))
            .cloned()
            .collect()
    }

    fn auth(&self) -> Result<Auth> {
        lock(&self.inner.auth)
            .clone()
            .filter(|auth| auth.state().is_signed_in())
            .context("Sign in to Zuse Cloud to link pull requests.")
    }

    pub async fn handle(&self, method: &str, params: Value) -> Result<Value> {
        let chat = params["chatId"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 200)
            .context("Choose a chat.")?
            .to_owned();
        let (repository, number) = match params["url"].as_str() {
            Some(url) => {
                let (repository, number) = zuse_protocol::pull_request_links::parse_url(url)
                    .context("That isn't a GitHub pull request link.")?;
                (repository, Some(number))
            }
            None => (
                params["repository"]
                    .as_str()
                    .context("Choose a pull request.")?
                    .to_owned(),
                params["number"].as_u64(),
            ),
        };
        match method {
            methods::CLAIM_PULL_REQUEST => {
                let source = params["source"]
                    .as_str()
                    .and_then(ClaimSource::parse)
                    .unwrap_or(ClaimSource::Manual);
                let branch = params["branch"].as_str().map(str::to_owned);
                let owned = self
                    .claim(&chat, &repository, number, branch, source)
                    .await?;
                Ok(json!({ "pullRequest": owned }))
            }
            methods::RELEASE_PULL_REQUEST => {
                let number = number.context("Choose a pull request.")?;
                self.request(
                    reqwest::Method::POST,
                    &["release"],
                    Some(json!({"chatKey": chat_key(&chat), "repository": repository, "number": number})),
                )
                .await?;
                self.inner.wake.notify_one();
                Ok(json!({ "released": true }))
            }
            methods::SET_PULL_REQUEST_POLICY => {
                let number = number.context("Choose a pull request.")?;
                let policy: EventPolicy = serde_json::from_value(params["policy"].clone())
                    .context("Invalid event settings.")?;
                let value = self
                    .request(
                        reqwest::Method::POST,
                        &["policy"],
                        Some(json!({"chatKey": chat_key(&chat), "repository": repository, "number": number, "policy": policy})),
                    )
                    .await?;
                self.inner.wake.notify_one();
                Ok(value)
            }
            _ => anyhow::bail!("Unknown pull request operation."),
        }
    }

    /// Make `chat` own a pull request: by number, or the open pull request
    /// whose head is `branch`.
    pub async fn claim(
        &self,
        chat: &str,
        repository: &str,
        number: Option<u64>,
        branch: Option<String>,
        source: ClaimSource,
    ) -> Result<OwnedPullRequest> {
        anyhow::ensure!(
            self.inner.workspace.chat(chat)?.is_some(),
            "This chat is not on this computer."
        );
        let value = self
            .request(
                reqwest::Method::POST,
                &["claim"],
                Some(json!({
                    "chatKey": chat_key(chat),
                    "repository": repository,
                    "number": number,
                    "branch": branch,
                    "source": source,
                })),
            )
            .await?;
        let owned: OwnedPullRequest = serde_json::from_value(value["pullRequest"].clone())?;
        self.remember(owned.clone());
        self.inner.wake.notify_one();
        Ok(owned)
    }

    async fn request(
        &self,
        verb: reqwest::Method,
        path: &[&str],
        body: Option<Value>,
    ) -> Result<Value> {
        let auth = self.auth()?;
        let mut full = vec!["v1", "cloud", "github", "pull-requests"];
        full.extend_from_slice(path);
        crate::cloud::pull_request_link_request(&auth, verb, &full, body).await
    }

    fn remember(&self, owned: OwnedPullRequest) {
        let mut state = lock(&self.inner.state);
        state
            .owned
            .retain(|pr| !(pr.repository.eq_ignore_ascii_case(&owned.repository) && pr.number == owned.number));
        state.owned.insert(0, owned);
        self.publish(&state);
        self.save(&state);
    }

    fn publish(&self, state: &State) {
        self.inner.updates.send_if_modified(|current| {
            if *current == state.owned {
                return false;
            }
            *current = state.owned.clone();
            true
        });
    }

    fn save(&self, state: &State) {
        let Ok(bytes) = serde_json::to_vec_pretty(state) else {
            return;
        };
        let temp = self.inner.path.with_extension("json.tmp");
        if std::fs::write(&temp, bytes)
            .and_then(|()| std::fs::rename(&temp, &self.inner.path))
            .is_err()
        {
            tracing::warn!("pull request links could not be saved");
        }
    }

    /// Activity kept for `chat`'s next turn, removed as it is handed over.
    fn take_context(&self, chat: &str) -> Option<String> {
        let mut state = lock(&self.inner.state);
        let notes = state.context.remove(chat)?;
        self.save(&state);
        (!notes.is_empty()).then(|| {
            format!(
                "Pull request activity since your last turn (no action was requested):\n\n{}",
                notes.join("\n\n")
            )
        })
    }

    async fn run(inner: Weak<Inner>, wake: Arc<Notify>) {
        let auth = match inner.upgrade() {
            Some(inner) => lock(&inner.auth).clone(),
            None => return,
        };
        let Some(auth) = auth else { return };
        // The feed is what tells us to read; it reconnects by itself.
        let mut feed: Option<crate::cloud::AccountFeed> = None;
        let mut next: Option<tokio::time::Instant> = Some(tokio::time::Instant::now());
        loop {
            if feed.is_none() {
                feed = crate::cloud::account_feed(&auth).ok();
            }
            let live = feed.as_ref().is_some_and(|f| *f.live.borrow());
            let fallback = tokio::time::Instant::now() + FALLBACK_EVERY;
            let deadline = match next {
                Some(at) if !live => at.min(fallback),
                Some(at) => at,
                None if !live => fallback,
                None => tokio::time::Instant::now() + Duration::from_secs(24 * 60 * 60),
            };
            let woke = match feed.as_mut() {
                Some(crate::cloud::AccountFeed { frames, live, .. }) => tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => Woke::Read,
                    _ = wake.notified() => Woke::Read,
                    frame = frames.recv() => match frame {
                        Ok(frame) if refresh_hint(&frame) => Woke::Read,
                        Ok(_) => Woke::Idle,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Woke::Read,
                        Err(_) => Woke::Lost,
                    },
                    // Reconnected: anything sent while it was down was missed.
                    changed = live.changed() => match changed {
                        Ok(()) if *live.borrow() => Woke::Read,
                        Ok(()) => Woke::Idle,
                        Err(_) => Woke::Lost,
                    },
                },
                None => tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => Woke::Read,
                    _ = wake.notified() => Woke::Read,
                },
            };
            let read = match woke {
                Woke::Read => true,
                Woke::Idle => false,
                Woke::Lost => {
                    feed = None;
                    true
                }
            };
            if !read {
                continue;
            }
            let Some(inner) = inner.upgrade() else { break };
            let this = Self { inner };
            if !auth.state().is_signed_in() {
                next = None;
                continue;
            }
            next = match this.sync().await {
                Ok(due) => due.map(|after| tokio::time::Instant::now() + after),
                Err(error) => {
                    tracing::debug!(%error, "pull request links deferred");
                    Some(tokio::time::Instant::now() + FALLBACK_EVERY)
                }
            };
        }
    }

    /// Refresh owned pull requests, then deliver what is due. Returns how
    /// long until held feedback is due, if any is held.
    async fn sync(&self) -> Result<Option<Duration>> {
        let value = self.request(reqwest::Method::GET, &["owned"], None).await?;
        let owned: Vec<OwnedPullRequest> = serde_json::from_value(value["pullRequests"].clone())?;
        {
            let mut state = lock(&self.inner.state);
            if state.owned != owned {
                state.owned = owned;
                self.publish(&state);
                self.save(&state);
            }
        }
        let value = self.request(reqwest::Method::GET, &["inbox"], None).await?;
        let events: Vec<InboxEvent> = serde_json::from_value(value["events"].clone())?;
        let plan = self.plan(events, Instant::now());
        let mut handled = plan.skip;
        for (chat, batch) in plan.deliver {
            match self.deliver(&chat, &batch) {
                Ok(()) => {
                    let mut state = lock(&self.inner.state);
                    for event in &batch {
                        state.delivered.push_back(event.event_id.clone());
                    }
                    while state.delivered.len() > DELIVERED_LIMIT {
                        state.delivered.pop_front();
                    }
                    self.save(&state);
                    handled.extend(batch.iter().map(|e| e.sequence));
                }
                Err(error) => tracing::warn!(%error, "pull request activity could not be delivered"),
            }
        }
        if !handled.is_empty() {
            lock(&self.inner.seen).retain(|_, at| at.elapsed() < Duration::from_secs(3600));
            self.request(
                reqwest::Method::POST,
                &["inbox", "ack"],
                Some(json!({ "sequences": handled })),
            )
            .await?;
        }
        Ok(plan.due)
    }

    /// Split inbox events into what to deliver now (per chat), what to
    /// acknowledge without delivering, and when held feedback is due.
    fn plan(&self, events: Vec<InboxEvent>, now: Instant) -> Plan {
        let delivered: HashSet<String> = lock(&self.inner.state).delivered.iter().cloned().collect();
        let watches = self.inner.pr_watches.subscribe().borrow().clone();
        let mut seen = lock(&self.inner.seen);
        let mut plan = Plan::default();
        let mut by_chat: BTreeMap<String, Vec<InboxEvent>> = BTreeMap::new();
        for event in events {
            let Some(chat) = event.chat_key.strip_prefix("chat:").map(str::to_owned) else {
                continue;
            };
            // Another computer's chat: leave it for that engine.
            if !self.inner.workspace.chat(&chat).ok().flatten().is_some() {
                continue;
            }
            // Delivered before a crash, or failing checks a CI watch with
            // auto-fix already repairs.
            let repaired = event.kind == PullRequestEventKind::ChecksFailed
                && watches.iter().any(|w| {
                    w.chat_id == chat
                        && w.enabled
                        && w.auto_fix
                        && zuse_protocol::pull_request_links::parse_url(&w.url)
                            .is_some_and(|(repo, n)| repo.eq_ignore_ascii_case(&event.repository) && n == event.number)
                });
            if delivered.contains(&event.event_id) || repaired || event.action == EventAction::Ignore {
                plan.skip.push(event.sequence);
                continue;
            }
            seen.entry(event.event_id.clone()).or_insert(now);
            by_chat.entry(chat).or_default().push(event);
        }
        for (chat, events) in by_chat {
            // Feedback waits until it has been quiet for a moment.
            let latest = events
                .iter()
                .filter(|e| e.kind.is_feedback())
                .filter_map(|e| seen.get(&e.event_id).copied())
                .max();
            let hold = latest.and_then(|at| FEEDBACK_QUIET.checked_sub(now.saturating_duration_since(at)));
            let (ready, held): (Vec<_>, Vec<_>) = events
                .into_iter()
                .partition(|e| !(hold.is_some() && e.kind.is_feedback()));
            if let Some(wait) = hold.filter(|_| !held.is_empty()) {
                plan.due = Some(plan.due.map_or(wait, |d: Duration| d.min(wait)));
            }
            if !ready.is_empty() {
                plan.deliver.push((chat, ready));
            }
        }
        plan
    }

    /// Put `events` into `chat`: one message, which starts a turn unless the
    /// activity is only context.
    fn deliver(&self, chat: &str, events: &[InboxEvent]) -> Result<()> {
        let row = self.inner.workspace.chat(chat)?.context("chat is gone")?;
        let doc = self.inner.docs.open(chat)?.doc_arc();
        let entries = doc.read_entries()?;
        let wake = should_wake(events, &entries, row.archived, chrono::Utc::now().timestamp_millis());
        let text = activity_text(events);
        let first = &events[0];
        let ids: Vec<&str> = events.iter().map(|e| e.event_id.as_str()).collect();
        let mut parts = vec![chat];
        parts.extend(ids);
        let id = format!("github-{}", short_id(&parts));
        let source = MessageSource::GitHub {
            text: summary(events),
            repository: first.repository.clone(),
            number: first.number,
            url: if first.url.is_empty() {
                format!("https://github.com/{}/pull/{}", first.repository, first.number)
            } else {
                first.url.clone()
            },
            event: first.kind,
            author: first.author.clone(),
        };
        if !wake {
            let mut state = lock(&self.inner.state);
            let notes = state.context.entry(chat.to_owned()).or_default();
            notes.push(text.clone());
            // Only the latest few matter by the time the agent reads them.
            if notes.len() > 20 {
                notes.drain(..notes.len() - 20);
            }
            self.save(&state);
        }
        let prompt = if wake {
            format!(
                "{text}\n\nYou own this pull request. Handle what needs a change, push the fix, and reply on GitHub with `gh` when that helps the reviewer."
            )
        } else {
            text
        };
        if !entries.iter().any(|e| e.id == id) {
            doc.push_message(&SessionMessageEntry {
                id: id.clone(),
                role: MessageRole::User,
                parts: vec![
                    MessagePart::Text {
                        id: "t0".into(),
                        text: prompt.clone(),
                    },
                    MessagePart::MessageSource {
                        id: "source".into(),
                        source,
                    },
                ],
                created_at: chrono::Utc::now().timestamp_millis(),
                device_id: self.inner.docs.device_id().to_owned(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })?;
        }
        if !wake {
            return Ok(());
        }
        let mut request = self
            .inner
            .docs
            .request_from_chat_row(chat, &prompt)
            .context("This chat can't take messages right now.")?;
        request.plan = false;
        self.inner.docs.queue_identified_command(
            chat,
            SessionCommandPayload::Run {
                request,
                message_id: id.clone(),
            },
            vec![],
            id,
        )?;
        Ok(())
    }

    /// Claim pull requests a chat opened or pushed to, once its turn ends.
    async fn follow_turns(inner: Weak<Inner>, mut statuses: watch::Receiver<Vec<zuse_protocol::Session>>) {
        let mut seen: HashMap<String, Option<String>> = statuses
            .borrow_and_update()
            .iter()
            .map(|s| (s.chat_id.clone(), s.last_completed_turn.clone()))
            .collect();
        while statuses.changed().await.is_ok() {
            let completed: Vec<(String, Option<String>)> = statuses
                .borrow_and_update()
                .iter()
                .filter(|s| {
                    s.last_completed_turn.is_some()
                        && seen.insert(s.chat_id.clone(), s.last_completed_turn.clone())
                            != Some(s.last_completed_turn.clone())
                })
                .map(|s| (s.chat_id.clone(), s.last_completed_turn.clone()))
                .collect();
            let Some(inner) = inner.upgrade() else { return };
            let this = Self { inner };
            if this.auth().is_err() {
                continue;
            }
            for (chat, _) in completed {
                if let Err(error) = this.claim_from_turn(&chat).await {
                    tracing::debug!(%error, "pull request not claimed");
                }
            }
        }
    }

    async fn claim_from_turn(&self, chat: &str) -> Result<()> {
        let entries = self.inner.docs.chat_entries(chat)?;
        if let Some(url) = crate::pr_watch::opened_pull_request(&entries)
            && let Some((repository, number)) = zuse_protocol::pull_request_links::parse_url(&url)
        {
            self.claim(chat, &repository, Some(number), None, ClaimSource::Created)
                .await?;
            return Ok(());
        }
        if !pushed(&entries) {
            return Ok(());
        }
        let row = self.inner.workspace.chat(chat)?.context("chat is gone")?;
        let cwd = row.cwd.context("chat has no checkout")?;
        let (repository, branch) = checkout_branch(&cwd).await?;
        match self
            .claim(chat, &repository, None, Some(branch), ClaimSource::Pushed)
            .await
        {
            // A pushed branch with no pull request yet is not an error.
            Err(error) if error.to_string() == crate::cloud::NO_PULL_REQUEST => Ok(()),
            other => other.map(|_| ()),
        }
    }
}

enum Woke {
    Read,
    Idle,
    /// The feed ended (signed out or replaced); subscribe again.
    Lost,
}

#[derive(Default)]
struct Plan {
    deliver: Vec<(String, Vec<InboxEvent>)>,
    skip: Vec<i64>,
    due: Option<Duration>,
}

/// Whether an account feed frame may mean new pull request activity.
fn refresh_hint(frame: &Value) -> bool {
    let data = &frame["data"];
    matches!(data["type"].as_str(), Some("ready" | "reset" | "revalidate"))
        || data["events"]
            .as_array()
            .is_some_and(|events| events.iter().any(|e| e["kind"] == FEED_KIND))
}

/// A person (not pull request activity) wrote this entry.
fn human(entry: &SessionMessageEntry) -> bool {
    entry.role == MessageRole::User
        && !entry
            .parts
            .iter()
            .any(|p| matches!(p, MessagePart::MessageSource { source, .. } if source.is_github()))
}

fn should_wake(
    events: &[InboxEvent],
    entries: &[SessionMessageEntry],
    archived: bool,
    now_ms: i64,
) -> bool {
    let waking: Vec<_> = events.iter().filter(|e| e.action == EventAction::Wake).collect();
    if archived || waking.is_empty() {
        return false;
    }
    let last_human = entries.iter().rposition(human);
    // Stop answering activity that nobody in the chat is following.
    let streak = entries[last_human.map_or(0, |i| i + 1)..]
        .iter()
        .filter(|e| e.role == MessageRole::User)
        .count();
    if streak >= WAKE_LIMIT {
        return false;
    }
    if waking.iter().all(|e| e.kind.is_state_change()) {
        return last_human
            .is_some_and(|i| now_ms - entries[i].created_at <= STATE_CHANGE_RECENCY_MS);
    }
    true
}

fn describe(event: &InboxEvent) -> String {
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
    };
    if !event.url.is_empty() {
        line.push_str(&format!("\n{}", event.url));
    }
    line
}

/// The full text the agent receives, grouped by pull request.
fn activity_text(events: &[InboxEvent]) -> String {
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

/// The short line the transcript shows.
fn summary(events: &[InboxEvent]) -> String {
    if let [event] = events {
        let line = describe(event);
        let line = line.lines().take(2).collect::<Vec<_>>().join(" ");
        return line.chars().take(280).collect();
    }
    let prs: HashSet<_> = events.iter().map(|e| (&e.repository, e.number)).collect();
    if prs.len() == 1 {
        format!("{} updates on {}#{}", events.len(), events[0].repository, events[0].number)
    } else {
        format!("{} updates on {} pull requests", events.len(), prs.len())
    }
}

/// The latest turn ran a successful `git push`.
fn pushed(entries: &[SessionMessageEntry]) -> bool {
    let turn = entries
        .iter()
        .rposition(|e| e.role == MessageRole::User)
        .map_or(0, |i| i + 1);
    entries[turn..].iter().flat_map(|e| &e.parts).any(|part| {
        matches!(part, MessagePart::Tool {
            call: zuse_protocol::ToolCall::Exec { command },
            resolved: true,
            is_error: false,
            ..
        } if runs_git_push(command))
    })
}

fn runs_git_push(command: &str) -> bool {
    command.split(['&', ';', '|', '\n']).any(|part| {
        let part = part.trim_start();
        part.starts_with("git push") || part.contains(" git push") || part.contains("git -C ") && part.contains(" push")
    })
}

/// The GitHub repository (`owner/name`) and current branch of a checkout.
async fn checkout_branch(cwd: &str) -> Result<(String, String)> {
    let git = |args: &'static [&'static str]| async move {
        let output = tokio::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .kill_on_drop(true)
            .output()
            .await?;
        anyhow::ensure!(output.status.success(), "git {} failed", args.join(" "));
        Ok::<_, anyhow::Error>(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    anyhow::ensure!(!branch.is_empty() && branch != "HEAD", "detached checkout");
    let remote = git(&["remote", "get-url", "origin"]).await?;
    let repository = github_repository(&remote).context("origin is not a GitHub repository")?;
    Ok((repository, branch))
}

/// `owner/name` from an HTTPS or SSH GitHub remote.
fn github_repository(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("git@github.com:"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    zuse_protocol::pull_request_links::valid_repository(path).then(|| path.to_owned())
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn entry(id: &str, at: i64, github: bool) -> SessionMessageEntry {
        let mut parts = vec![MessagePart::Text {
            id: "t0".into(),
            text: "hi".into(),
        }];
        if github {
            parts.push(MessagePart::MessageSource {
                id: "source".into(),
                source: MessageSource::GitHub {
                    text: "x".into(),
                    repository: "acme/app".into(),
                    number: 7,
                    url: String::new(),
                    event: PullRequestEventKind::Comment,
                    author: None,
                },
            });
        }
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts,
            created_at: at,
            device_id: "d".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn feedback_wakes_unless_archived_or_only_context() {
        let comment = [event(PullRequestEventKind::Comment, EventAction::Wake)];
        let history = [entry("u", 0, false)];
        assert!(should_wake(&comment, &history, false, 1));
        assert!(!should_wake(&comment, &history, true, 1));
        let context = [event(PullRequestEventKind::Comment, EventAction::Context)];
        assert!(!should_wake(&context, &history, false, 1));
    }

    #[test]
    fn state_changes_only_wake_recently_used_chats() {
        let merged = [event(PullRequestEventKind::Merged, EventAction::Wake)];
        let history = [entry("u", 0, false)];
        assert!(should_wake(&merged, &history, false, STATE_CHANGE_RECENCY_MS));
        assert!(!should_wake(&merged, &history, false, STATE_CHANGE_RECENCY_MS + 1));
        assert!(!should_wake(&merged, &[], false, 0));
    }

    #[test]
    fn a_long_run_of_activity_without_a_person_stops_waking() {
        let comment = [event(PullRequestEventKind::Comment, EventAction::Wake)];
        let mut history = vec![entry("u", 0, false)];
        for i in 0..WAKE_LIMIT - 1 {
            history.push(entry(&format!("g{i}"), 0, true));
        }
        assert!(should_wake(&comment, &history, false, 1));
        history.push(entry("last", 0, true));
        assert!(!should_wake(&comment, &history, false, 1));
        // A person writing in the chat resets it.
        history.push(entry("again", 0, false));
        assert!(should_wake(&comment, &history, false, 1));
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
    fn hints_and_resets_trigger_a_read() {
        assert!(refresh_hint(&json!({"data":{"events":[{"kind":"pull-request","resourceId":"chat:c"}]}})));
        assert!(refresh_hint(&json!({"data":{"type":"ready"}})));
        assert!(!refresh_hint(&json!({"data":{"events":[{"kind":"billing","resourceId":"a"}]}})));
    }

    #[test]
    fn git_pushes_are_recognised() {
        assert!(runs_git_push("git push -u origin fix"));
        assert!(runs_git_push("cargo test && git push"));
        assert!(runs_git_push("git -C /repo push origin HEAD"));
        assert!(!runs_git_push("git pull"));
        assert!(!runs_git_push("echo pushing"));
    }

    #[test]
    fn github_remotes_parse() {
        assert_eq!(github_repository("https://github.com/acme/app.git").as_deref(), Some("acme/app"));
        assert_eq!(github_repository("git@github.com:acme/app.git").as_deref(), Some("acme/app"));
        assert_eq!(github_repository("ssh://git@github.com/acme/app").as_deref(), Some("acme/app"));
        assert_eq!(github_repository("https://gitlab.com/acme/app.git"), None);
    }
}
