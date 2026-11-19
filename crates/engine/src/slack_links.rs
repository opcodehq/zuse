//! Chats linked to Slack threads.
//!
//! The cloud service owns the Slack installation and posts on our behalf; this
//! engine decides what to post. It keeps a small per-profile record of which of
//! its chats are linked, posts each newly finished reply of a synced chat into
//! its thread (as the chat's Zapper when it belongs to one), and turns replies
//! written in a linked thread into ordinary queued messages of that chat.
//!
//! Only chats hosted on this engine are mirrored. Delivery is idempotent on
//! both sides: posts are keyed by the completed turn, and inbound messages by
//! their Slack event, so a crash between steps never duplicates either.

use crate::cloud::identity::CloudIdentity;
use crate::{DocHost, SessionsEngine, WorkspaceHost};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use zuse_doc::{SessionCommandPayload, SessionMessageEntry};
use zuse_model::{MessagePart, MessageRole, MessageStatus};
use zuse_protocol::slack::{ChatLink, InboxMessage, Persona};
use zuse_protocol::{SessionStatus, methods};

/// How often the inbox is checked while at least one chat is synced.
const INBOX_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct SlackLinks {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    state: Mutex<State>,
    zots: crate::zots::Zots,
    workspace: WorkspaceHost,
    docs: DocHost,
    sessions: SessionsEngine,
    identity: Mutex<Option<CloudIdentity>>,
    started: OnceLock<()>,
    wake: Arc<tokio::sync::Notify>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Zapper worker chats whose automatic link failed this session; retried after restart.
    auto_link_failed: Mutex<HashSet<String>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(default)]
    chats: BTreeMap<String, Local>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Local {
    team_id: String,
    channel_id: String,
    sync: bool,
    /// The last completed turn already posted (or present when the link was made).
    #[serde(default)]
    posted_turn: Option<String>,
}

fn chat_key(chat_id: &str) -> String {
    format!("chat:{chat_id}")
}

fn chat_id_param(params: &Value) -> Result<String> {
    params["chatId"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 200)
        .map(str::to_owned)
        .context("Choose a chat to link.")
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
        methods::SLACK_CHANNELS
            | methods::SLACK_CHAT_LINK
            | methods::SLACK_CHAT_LINK_CREATE
            | methods::SLACK_CHAT_LINK_UPDATE
            | methods::SLACK_CHAT_LINK_REMOVE
    )
}

impl SlackLinks {
    pub fn new(
        store_root: &std::path::Path,
        zots: crate::zots::Zots,
        workspace: WorkspaceHost,
        docs: DocHost,
        sessions: SessionsEngine,
    ) -> Self {
        let path = store_root.join("slack-links.json");
        let state = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                path,
                state: Mutex::new(state),
                zots,
                workspace,
                docs,
                sessions,
                identity: Mutex::new(None),
                started: OnceLock::new(),
                wake: Arc::new(tokio::sync::Notify::new()),
                task: Mutex::new(None),
                auto_link_failed: Mutex::new(HashSet::new()),
            }),
        }
    }

    /// Give the mirror the account it acts for and start it once.
    pub fn attach(&self, identity: CloudIdentity) {
        *lock(&self.inner.identity) = Some(identity);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self.inner.started.set(()).is_err() {
            return;
        }
        let inner = Arc::downgrade(&self.inner);
        let wake = self.inner.wake.clone();
        let statuses = self.inner.sessions.watch_sessions();
        *lock(&self.inner.task) = Some(runtime.spawn(Self::run(inner, wake, statuses)));
    }

    pub async fn shutdown(&self) {
        let task = lock(&self.inner.task).take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }

    pub async fn handle(
        &self,
        identity: &CloudIdentity,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        self.attach(identity.clone());
        let account = identity.user_id();
        let data = match method {
            methods::SLACK_CHANNELS => {
                crate::cloud::slack_link_request(
                    identity,
                    reqwest::Method::GET,
                    &["v1", "cloud", "slack", "channels"],
                    None,
                    &[],
                )
                .await?
            }
            methods::SLACK_CHAT_LINK => {
                let chat = chat_id_param(&params)?;
                let value = crate::cloud::slack_link_request(
                    identity,
                    reqwest::Method::GET,
                    &["v1", "cloud", "slack", "links"],
                    None,
                    &[("chatKey".into(), chat_key(&chat))],
                )
                .await?;
                // The cloud record wins: a link removed elsewhere stops mirroring here.
                match serde_json::from_value::<Option<ChatLink>>(value["link"].clone())? {
                    Some(link) => self.remember(&chat, &link, false),
                    None => self.forget(&chat),
                }
                value
            }
            methods::SLACK_CHAT_LINK_CREATE => {
                let chat = chat_id_param(&params)?;
                let team = params["teamId"]
                    .as_str()
                    .context("Choose a Slack channel.")?;
                let channel = params["channelId"]
                    .as_str()
                    .context("Choose a Slack channel.")?;
                let link = self.create(identity, &chat, team, channel).await?;
                json!({ "link": link })
            }
            methods::SLACK_CHAT_LINK_UPDATE => {
                let chat = chat_id_param(&params)?;
                let sync = params["sync"].as_bool().context("Missing sync setting.")?;
                let value = crate::cloud::slack_link_request(
                    identity,
                    reqwest::Method::POST,
                    &["v1", "cloud", "slack", "links", "update"],
                    Some(json!({ "chatKey": chat_key(&chat), "sync": sync })),
                    &[],
                )
                .await?;
                let link: ChatLink = serde_json::from_value(value["link"].clone())?;
                // Turning sync back on must not post the replies written while it was off.
                self.remember(&chat, &link, sync);
                value
            }
            methods::SLACK_CHAT_LINK_REMOVE => {
                let chat = chat_id_param(&params)?;
                let value = crate::cloud::slack_link_request(
                    identity,
                    reqwest::Method::POST,
                    &["v1", "cloud", "slack", "links", "remove"],
                    Some(json!({ "chatKey": chat_key(&chat) })),
                    &[],
                )
                .await?;
                self.forget(&chat);
                value
            }
            _ => anyhow::bail!("Unknown Slack operation."),
        };
        self.inner.wake.notify_one();
        Ok(json!({ "accountId": account, "organizationId": Value::Null, "data": data }))
    }

    async fn create(
        &self,
        identity: &CloudIdentity,
        chat: &str,
        team: &str,
        channel: &str,
    ) -> Result<ChatLink> {
        ensure!(
            self.inner.workspace.chat(chat)?.is_some(),
            "This chat is not on this computer."
        );
        let persona = self.persona(chat);
        let title = self.title(chat, persona.as_ref());
        let value = crate::cloud::slack_link_request(
            identity,
            reqwest::Method::POST,
            &["v1", "cloud", "slack", "links"],
            Some(json!({
                "chatKey": chat_key(chat),
                "teamId": team,
                "channelId": channel,
                "title": title,
                "persona": persona,
            })),
            &[],
        )
        .await?;
        let link: ChatLink = serde_json::from_value(value["link"].clone())?;
        self.remember(chat, &link, true);
        Ok(link)
    }

    /// Record a link. `fresh` marks everything already finished as posted, so
    /// linking (or re-enabling sync) only mirrors replies from now on.
    fn remember(&self, chat: &str, link: &ChatLink, fresh: bool) {
        let current = self.completed_turn(chat);
        let mut state = lock(&self.inner.state);
        let previous = state.chats.get(chat).cloned();
        let posted_turn = match previous {
            Some(previous) if !fresh => previous.posted_turn,
            _ => current,
        };
        state.chats.insert(
            chat.to_owned(),
            Local {
                team_id: link.team_id.clone(),
                channel_id: link.channel_id.clone(),
                sync: link.sync,
                posted_turn,
            },
        );
        self.save(&state);
    }

    fn forget(&self, chat: &str) {
        let mut state = lock(&self.inner.state);
        if state.chats.remove(chat).is_some() {
            self.save(&state);
        }
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
            tracing::warn!("Slack link state could not be saved");
        }
    }

    /// Chats that belong to a Zapper post under its name and avatar.
    fn persona(&self, chat: &str) -> Option<Persona> {
        let zot = self.zapper_of(chat)?;
        Some(Persona {
            name: zot.config.name.chars().take(80).collect(),
            avatar: Some(zot.config.avatar.clone().unwrap_or_else(|| {
                zuse_protocol::presentation::zot_mascots::default_avatar(&zot.id)
            })),
        })
    }

    fn zapper_of(&self, chat: &str) -> Option<zuse_protocol::zots::Zot> {
        self.inner.zots.list().ok()?.into_iter().find(|zot| {
            zot.coordinator_chat_id == chat || zot.workers.iter().any(|w| w.chat_id == chat)
        })
    }

    fn title(&self, chat: &str, persona: Option<&Persona>) -> String {
        let row = self.inner.workspace.chat(chat).ok().flatten();
        let title = row
            .as_ref()
            .and_then(|c| c.title.clone())
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                self.zapper_of(chat)
                    .filter(|zot| zot.coordinator_chat_id == chat)
                    .map(|zot| zot.config.name)
            })
            .unwrap_or_else(|| "Zuse chat".into());
        match persona {
            Some(persona) if persona.name != title => format!("{title} · {}", persona.name),
            _ => title,
        }
    }

    fn completed_turn(&self, chat: &str) -> Option<String> {
        self.inner
            .sessions
            .watch_sessions()
            .borrow()
            .iter()
            .find(|s| s.chat_id == chat)
            .and_then(|s| s.last_completed_turn.clone())
    }

    /// Everything the assistant wrote after the last person's message.
    fn last_reply(&self, chat: &str) -> Option<String> {
        let entries = self
            .inner
            .docs
            .open(chat)
            .ok()?
            .doc_arc()
            .read_entries()
            .ok()?;
        let start = entries
            .iter()
            .rposition(|e| e.role == MessageRole::User)
            .map_or(0, |i| i + 1);
        let text = entries[start..]
            .iter()
            .filter(|e| e.role == MessageRole::Assistant)
            .flat_map(|e| e.parts.iter())
            .filter_map(|part| match part {
                MessagePart::Text { text, .. } => Some(text.trim()),
                _ => None,
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!text.is_empty()).then_some(text)
    }

    async fn run(
        inner: Weak<Inner>,
        wake: Arc<tokio::sync::Notify>,
        mut statuses: tokio::sync::watch::Receiver<Vec<zuse_protocol::Session>>,
    ) {
        let mut tick = tokio::time::interval(INBOX_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let mut poll = false;
            tokio::select! {
                _ = tick.tick() => poll = true,
                _ = wake.notified() => poll = true,
                changed = statuses.changed() => if changed.is_err() { break },
            }
            let Some(inner) = inner.upgrade() else { break };
            let this = Self { inner };
            let identity = lock(&this.inner.identity).clone();
            let Some(identity) = identity else { continue };
            if !identity.state().is_signed_in() || lock(&this.inner.state).chats.is_empty() {
                continue;
            }
            if let Err(error) = this.mirror(&identity).await {
                tracing::debug!(%error, "Slack mirror deferred");
            }
            if poll {
                this.link_zapper_workers(&identity).await;
                if let Err(error) = this.receive(&identity).await {
                    tracing::debug!(%error, "Slack inbox deferred");
                }
            }
        }
    }

    /// Post each synced chat's newly completed turn.
    async fn mirror(&self, identity: &CloudIdentity) -> Result<()> {
        let sessions = self.inner.sessions.watch_sessions().borrow().clone();
        let due: Vec<(String, String)> = {
            let state = lock(&self.inner.state);
            state
                .chats
                .iter()
                .filter(|(_, local)| local.sync)
                .filter_map(|(chat, local)| {
                    let session = sessions.iter().find(|s| &s.chat_id == chat)?;
                    let turn = session.last_completed_turn.clone()?;
                    (session.status != SessionStatus::Working
                        && local.posted_turn.as_ref() != Some(&turn))
                    .then(|| (chat.clone(), turn))
                })
                .collect()
        };
        for (chat, turn) in due {
            if let Some(text) = self.last_reply(&chat) {
                let value = crate::cloud::slack_link_request(
                    identity,
                    reqwest::Method::POST,
                    &["v1", "cloud", "slack", "links", "post"],
                    Some(json!({
                        "chatKey": chat_key(&chat),
                        "messageId": short_id(&[&chat, &turn]),
                        "text": zuse_protocol::slack::mirror_text(&text),
                        "persona": self.persona(&chat),
                    })),
                    &[],
                )
                .await;
                match value {
                    Ok(_) => {}
                    // Unlinked elsewhere: stop mirroring instead of retrying forever.
                    Err(error) if error.to_string().contains("no longer linked") => {
                        self.forget(&chat);
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            let mut state = lock(&self.inner.state);
            if let Some(local) = state.chats.get_mut(&chat) {
                local.posted_turn = Some(turn);
                self.save(&state);
            }
        }
        Ok(())
    }

    /// Worker threads of a linked Zapper get their own thread in the Zapper's channel.
    async fn link_zapper_workers(&self, identity: &CloudIdentity) {
        let Ok(zots) = self.inner.zots.list() else {
            return;
        };
        let device = self.inner.docs.device_id().to_owned();
        for zot in zots {
            let Some(home) = lock(&self.inner.state)
                .chats
                .get(&zot.coordinator_chat_id)
                .filter(|local| local.sync)
                .cloned()
            else {
                continue;
            };
            for worker in zot.workers.iter().filter(|w| w.device_id == device) {
                if lock(&self.inner.state).chats.contains_key(&worker.chat_id)
                    || lock(&self.inner.auto_link_failed).contains(&worker.chat_id)
                {
                    continue;
                }
                if let Err(error) = self
                    .create(identity, &worker.chat_id, &home.team_id, &home.channel_id)
                    .await
                {
                    tracing::debug!(%error, "Zapper worker could not be linked to Slack");
                    lock(&self.inner.auto_link_failed).insert(worker.chat_id.clone());
                }
            }
        }
    }

    /// Deliver replies written in linked threads, then acknowledge them.
    async fn receive(&self, identity: &CloudIdentity) -> Result<()> {
        if !lock(&self.inner.state).chats.values().any(|l| l.sync) {
            return Ok(());
        }
        let value = crate::cloud::slack_link_request(
            identity,
            reqwest::Method::GET,
            &["v1", "cloud", "slack", "inbox"],
            None,
            &[],
        )
        .await?;
        let messages: Vec<InboxMessage> = serde_json::from_value(value["messages"].clone())?;
        let mut handled = Vec::new();
        for message in messages {
            let Some(chat) = message.chat_key.strip_prefix("chat:") else {
                continue;
            };
            // Another computer's chat: leave it for that engine.
            let synced = lock(&self.inner.state)
                .chats
                .get(chat)
                .is_some_and(|l| l.sync);
            if !synced {
                continue;
            }
            match self.deliver(chat, &message) {
                Ok(()) => handled.push(message.sequence),
                Err(error) => tracing::warn!(%error, "Slack reply could not be queued"),
            }
        }
        if !handled.is_empty() {
            crate::cloud::slack_link_request(
                identity,
                reqwest::Method::POST,
                &["v1", "cloud", "slack", "inbox", "ack"],
                Some(json!({ "sequences": handled })),
                &[],
            )
            .await?;
        }
        Ok(())
    }

    fn deliver(&self, chat: &str, message: &InboxMessage) -> Result<()> {
        let text = message.text.trim();
        ensure!(!text.is_empty(), "empty Slack reply");
        let source = zuse_protocol::slack::MessageSource::Slack {
            text: text.to_owned(),
            author: message.author.clone(),
            channel: message.channel.clone(),
        };
        let prompt = format!(
            "Reply from {} in this chat's linked Slack thread:\n\n{text}",
            source.sender_name()
        );
        let id = format!("slack-{}", short_id(&[chat, &message.event_id]));
        let mut request = self
            .inner
            .docs
            .request_from_chat_row(chat, &prompt)
            .context("This chat can't take messages right now.")?;
        request.plan = false;
        let doc = self.inner.docs.open(chat)?.doc_arc();
        // Written first, with its Slack attribution; the run reuses this entry by id.
        if !doc.read_entries()?.iter().any(|e| e.id == id) {
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

    #[test]
    fn post_ids_are_stable_per_chat_and_turn() {
        assert_eq!(short_id(&["a", "t1"]), short_id(&["a", "t1"]));
        assert_ne!(short_id(&["a", "t1"]), short_id(&["a", "t2"]));
        // Separators keep ("ab","c") and ("a","bc") apart.
        assert_ne!(short_id(&["ab", "c"]), short_id(&["a", "bc"]));
        assert_eq!(short_id(&["a"]).len(), 32);
    }

    #[test]
    fn chat_ids_are_bounded() {
        assert!(chat_id_param(&json!({"chatId": "c1"})).is_ok());
        assert!(chat_id_param(&json!({"chatId": ""})).is_err());
        assert!(chat_id_param(&json!({"chatId": "x".repeat(201)})).is_err());
        assert!(chat_id_param(&json!({})).is_err());
    }
}
