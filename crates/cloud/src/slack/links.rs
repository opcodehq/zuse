//! Zuse chats linked to Slack threads. Linking posts a parent message in a
//! channel; the chat's finished replies are mirrored into that thread and the
//! linked Slack member's thread replies wait in an inbox the engine polls.
use super::*;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use sqlx::Row;
use std::collections::HashMap;
use zuse_protocol::{
    slack::{Channel, ChatLink, LinkRequest, Persona, PostRequest, avatar_id_ok, mirror_text},
    zappers::ZapperAvatar,
};

/// Largest accepted chat key, message ID or Slack identifier.
const KEY_LIMIT: usize = 256;
const TITLE_LIMIT: usize = 200;
const INBOX_PAGE: i64 = 50;
const ACK_LIMIT: usize = 100;
/// `conversations.list` pages; Slack responses stay within the bounded body.
const CHANNEL_PAGE: &str = "100";
const CHANNEL_PAGES: usize = 10;
/// A post claim without a timestamp older than this is assumed lost.
const POST_CLAIM_MS: i64 = 60_000;
/// Unread thread replies are dropped after a week.
pub(super) const INBOX_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

fn key_ok(value: &str) -> bool {
    !value.is_empty() && value.len() <= KEY_LIMIT && !value.chars().any(char::is_control)
}
fn slack_id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
/// Escape Slack's control characters so titles, names and replies cannot
/// produce mentions such as `<!channel>` or disguised links.
pub(super) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
pub(super) fn title(raw: &str) -> String {
    let title: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(TITLE_LIMIT)
        .collect();
    let title = title.trim();
    if title.is_empty() {
        "Zuse chat".into()
    } else {
        title.into()
    }
}
fn inbox_context(link: &str, ts: &str) -> String {
    format!("zuse-slack-inbox-v1\n{link}\n{ts}")
}
fn display_name(user: &Value) -> Option<String> {
    user["profile"]["display_name"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .or(user["real_name"].as_str().filter(|v| !v.trim().is_empty()))
        .or(user["name"].as_str().filter(|v| !v.trim().is_empty()))
        .map(str::to_owned)
}

/// A thread reply that may belong to a linked chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LinkedReply {
    pub team: String,
    pub id: String,
    pub channel: String,
    pub user: String,
    pub ts: String,
    pub thread: String,
    pub text: String,
}
/// What the events endpoint does with a candidate thread reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// Not a linked thread: today's mention/DM handling applies.
    Default,
    /// A synced linked thread, but not something the chat should receive.
    Ignore,
    /// Queue for the linked chat's engine; no Slack task starts.
    Inbox,
}
/// Parse a human reply inside a channel thread. Does not touch the database.
pub(super) fn linked_reply(body: &Value, bot: &str) -> Option<LinkedReply> {
    if body["type"] != "event_callback" {
        return None;
    }
    let ev = &body["event"];
    if ev["bot_id"].as_str().is_some_and(|id| !id.is_empty())
        || ev["subtype"]
            .as_str()
            .is_some_and(|v| v != "file_share" && v != "thread_broadcast")
    {
        return None;
    }
    let is_message =
        ev["type"] == "message" && matches!(ev["channel_type"].as_str(), Some("channel" | "group"));
    if !is_message && ev["type"] != "app_mention" {
        return None;
    }
    let string = |v: &Value| {
        v.as_str()
            .filter(|s| !s.is_empty() && s.len() <= KEY_LIMIT)
            .map(str::to_owned)
    };
    let ts = string(&ev["ts"])?;
    let thread = string(&ev["thread_ts"]).filter(|thread| *thread != ts)?;
    let user = string(&ev["user"]).filter(|user| user != bot)?;
    let raw = ev["text"].as_str().unwrap_or("");
    let mut text = if raw.len() > 40_000 {
        String::new()
    } else {
        raw.replace(&format!("<@{bot}>"), "").trim().to_owned()
    };
    if !text.is_empty() && ev["files"].as_array().is_some_and(|f| !f.is_empty()) {
        text.push_str("\n\n(Slack attachments are not included.)");
    }
    Some(LinkedReply {
        team: string(&body["team_id"])?,
        id: string(&body["event_id"])?,
        channel: string(&ev["channel"])?,
        user,
        ts,
        thread,
        text,
    })
}
/// `link` is `None` when no synced link owns the thread, otherwise whether the
/// sender is a Slack member linked to the link's Zuse account and actor.
pub(super) fn route(reply: &LinkedReply, link: Option<bool>) -> Route {
    match link {
        None => Route::Default,
        Some(false) => Route::Ignore,
        Some(true) if reply.text.is_empty() => Route::Ignore,
        Some(true) => Route::Inbox,
    }
}

#[derive(Serialize, Deserialize)]
struct Queued {
    text: String,
    user: String,
}

struct Install {
    token: String,
    team_name: String,
}
struct LinkRow {
    link_id: String,
    team_id: String,
    team_name: String,
    channel_id: String,
    channel_name: String,
    thread_ts: String,
    sync: bool,
}
impl LinkRow {
    fn view(&self, permalink: Option<String>) -> ChatLink {
        ChatLink {
            team_id: self.team_id.clone(),
            team_name: self.team_name.clone(),
            channel_id: self.channel_id.clone(),
            channel_name: self.channel_name.clone(),
            thread_ts: self.thread_ts.clone(),
            sync: self.sync,
            permalink,
        }
    }
}

impl Slack {
    /// Route a thread reply in a linked thread to its chat's inbox. Returns
    /// whether the event was consumed; unlinked threads are left untouched.
    /// Database only: the events endpoint must acknowledge within 3 seconds.
    pub(super) async fn intercept_linked_reply(
        &self,
        body: &Value,
        bot: &str,
    ) -> Result<bool, ApiError> {
        let Some(reply) = linked_reply(body, bot) else {
            return Ok(false);
        };
        let found: Option<(String, bool)> = sqlx::query_as("SELECT l.link_id,EXISTS(SELECT 1 FROM zuse_rust_slack_members m WHERE m.team_id=l.team_id AND m.user_id=$4 AND m.account_id=l.account_id AND m.actor_id=l.actor_id) FROM zuse_rust_slack_links l WHERE l.team_id=$1 AND l.channel_id=$2 AND l.thread_ts=$3 AND l.sync")
            .bind(&reply.team).bind(&reply.channel).bind(&reply.thread).bind(&reply.user)
            .fetch_optional(&self.pool).await?;
        match route(&reply, found.as_ref().map(|(_, member)| *member)) {
            Route::Default => Ok(false),
            Route::Ignore => Ok(true),
            Route::Inbox => {
                let (link, _) = found.ok_or_else(unavailable)?;
                let payload = self.seal(
                    &inbox_context(&link, &reply.ts),
                    &Queued {
                        text: reply.text,
                        user: reply.user,
                    },
                )?;
                sqlx::query("INSERT INTO zuse_rust_slack_inbox(link_id,event_id,message_ts,payload,created_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                    .bind(&link).bind(&reply.id).bind(&reply.ts).bind(payload).bind(now())
                    .execute(&self.pool).await?;
                Ok(true)
            }
        }
    }
    async fn install(&self, team: &str) -> Result<Install, ApiError> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT credentials,team_name FROM zuse_rust_slack_installs WHERE team_id=$1",
        )
        .bind(team)
        .fetch_optional(&self.pool)
        .await?;
        let (credentials, team_name) =
            row.ok_or_else(|| ApiError::forbidden("slack_member_required"))?;
        let credentials: Value = self.open(&format!("slack-install:{team}"), &credentials)?;
        Ok(Install {
            token: credentials["token"]
                .as_str()
                .ok_or_else(unavailable)?
                .to_owned(),
            team_name,
        })
    }
    async fn link_row(&self, access: &Access, chat_key: &str) -> Result<Option<LinkRow>, ApiError> {
        let row = sqlx::query("SELECT l.link_id,l.team_id,i.team_name,l.channel_id,l.channel_name,l.thread_ts,l.sync FROM zuse_rust_slack_links l JOIN zuse_rust_slack_installs i USING(team_id) WHERE l.account_id=$1 AND l.actor_id=$2 AND l.chat_key=$3")
            .bind(&access.owner_id).bind(&access.actor.account_id).bind(chat_key)
            .fetch_optional(&self.pool).await?;
        Ok(row.map(|r| LinkRow {
            link_id: r.get("link_id"),
            team_id: r.get("team_id"),
            team_name: r.get("team_name"),
            channel_id: r.get("channel_id"),
            channel_name: r.get("channel_name"),
            thread_ts: r.get("thread_ts"),
            sync: r.get("sync"),
        }))
    }
    async fn permalink(&self, link: &LinkRow) -> Option<String> {
        let install = self.install(&link.team_id).await.ok()?;
        self.api(
            &install.token,
            "chat.getPermalink",
            json!({"channel":link.channel_id,"message_ts":link.thread_ts}),
        )
        .await
        .ok()?["permalink"]
            .as_str()
            .filter(|v| v.starts_with("https://") && v.len() <= 2048)
            .map(str::to_owned)
    }
    /// Slack `username`/`icon_url` overrides; needs `chat:write.customize`.
    fn persona_fields(&self, persona: Option<&Persona>, body: &mut Value) {
        let Some(persona) = persona else { return };
        body["username"] = json!(persona.name.trim());
        if let Some(avatar) = &persona.avatar {
            body["icon_url"] = json!(avatar_url(&http::public_base(self), avatar));
        }
    }
    /// Post, joining a public channel once if Slack says the bot is not in it.
    async fn post_message(
        &self,
        token: &str,
        body: Value,
        public: bool,
    ) -> Result<Value, ApiError> {
        let channel = body["channel"].clone();
        match self.call(token, "chat.postMessage", body.clone()).await? {
            Ok(value) => Ok(value),
            Err(error) if error == "not_in_channel" && public => {
                self.api(token, "conversations.join", json!({"channel":channel}))
                    .await?;
                self.api(token, "chat.postMessage", body).await
            }
            Err(error) => Err(slack_error(&error)),
        }
    }
}
fn slack_error(error: &str) -> ApiError {
    match error {
        "missing_scope" | "not_in_channel" | "channel_not_found" | "is_archived" => {
            ApiError::forbidden("slack_channel_access_unavailable")
        }
        _ => unavailable(),
    }
}
pub(super) fn avatar_url(base: &str, avatar: &ZapperAvatar) -> String {
    format!(
        "{base}/slack/avatars/{}/{}.png",
        avatar.mascot, avatar.color
    )
}

pub(super) async fn channels(
    State(s): State<Slack>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    let teams: Vec<String> = sqlx::query_scalar("SELECT DISTINCT team_id FROM zuse_rust_slack_members WHERE account_id=$1 AND actor_id=$2 ORDER BY team_id")
        .bind(&access.owner_id).bind(&access.actor.account_id)
        .fetch_all(&s.pool).await?;
    let mut channels = Vec::new();
    let mut failure = None;
    let mut listed = 0;
    for team in &teams {
        match list_channels(&s, team).await {
            Ok(found) => {
                listed += 1;
                channels.extend(found);
            }
            // One unreachable workspace must not hide the others.
            Err(error) => {
                tracing::warn!(code = error.code, "Slack channel list unavailable");
                failure = Some(error);
            }
        }
    }
    if listed == 0
        && let Some(error) = failure
    {
        return Err(error);
    }
    channels.sort_by(|a: &Channel, b: &Channel| {
        (
            a.team_name.to_lowercase(),
            &a.team_id,
            a.name.to_lowercase(),
        )
            .cmp(&(
                b.team_name.to_lowercase(),
                &b.team_id,
                b.name.to_lowercase(),
            ))
    });
    Ok(Json(json!({ "channels": channels })))
}
async fn list_channels(s: &Slack, team: &str) -> Result<Vec<Channel>, ApiError> {
    let install = s.install(team).await?;
    let mut channels = Vec::new();
    let mut cursor = String::new();
    for _ in 0..CHANNEL_PAGES {
        let mut query = json!({"types":"public_channel,private_channel","exclude_archived":"true","limit":CHANNEL_PAGE});
        if !cursor.is_empty() {
            query["cursor"] = json!(cursor);
        }
        let page = s.api(&install.token, "conversations.list", query).await?;
        for channel in page["channels"].as_array().into_iter().flatten() {
            let (Some(id), Some(name)) = (channel["id"].as_str(), channel["name"].as_str()) else {
                continue;
            };
            let private = channel["is_private"] == true;
            // A bot token lists only private channels it belongs to; check anyway.
            if !slack_id_ok(id)
                || channel["is_archived"] == true
                || (private && channel["is_member"] != true)
            {
                continue;
            }
            channels.push(Channel {
                team_id: team.to_owned(),
                team_name: install.team_name.clone(),
                channel_id: id.to_owned(),
                name: name.to_owned(),
                private,
            });
        }
        cursor = page["response_metadata"]["next_cursor"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        if cursor.is_empty() {
            break;
        }
    }
    Ok(channels)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LinkQuery {
    chat_key: String,
}
pub(super) async fn get_link(
    State(s): State<Slack>,
    headers: HeaderMap,
    Query(q): Query<LinkQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    if !key_ok(&q.chat_key) {
        return Err(invalid());
    }
    let connected: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM zuse_rust_slack_members WHERE account_id=$1 AND actor_id=$2)",
    )
    .bind(&access.owner_id)
    .bind(&access.actor.account_id)
    .fetch_one(&s.pool)
    .await?;
    let link = match s.link_row(&access, &q.chat_key).await? {
        Some(row) => {
            let permalink = s.permalink(&row).await;
            Some(row.view(permalink))
        }
        None => None,
    };
    Ok(Json(json!({"link":link,"connected":connected})))
}

pub(super) async fn create(
    State(s): State<Slack>,
    headers: HeaderMap,
    Json(q): Json<LinkRequest>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    if !key_ok(&q.chat_key) || !slack_id_ok(&q.team_id) || !slack_id_ok(&q.channel_id) {
        return Err(invalid());
    }
    if q.persona.as_ref().is_some_and(|p| !p.is_valid()) {
        return Err(ApiError::bad_request("slack_persona_invalid"));
    }
    let title = title(&q.title);
    if let Some(row) = s.link_row(&access, &q.chat_key).await? {
        if row.team_id != q.team_id || row.channel_id != q.channel_id {
            return Err(ApiError::conflict("slack_chat_already_linked"));
        }
        let permalink = s.permalink(&row).await;
        return Ok(Json(json!({"link":row.view(permalink)})));
    }
    let user: Option<String> = sqlx::query_scalar("SELECT user_id FROM zuse_rust_slack_members WHERE team_id=$1 AND account_id=$2 AND actor_id=$3 ORDER BY user_id LIMIT 1")
        .bind(&q.team_id).bind(&access.owner_id).bind(&access.actor.account_id)
        .fetch_optional(&s.pool).await?;
    let user = user.ok_or_else(|| ApiError::forbidden("slack_member_required"))?;
    let install = s.install(&q.team_id).await?;
    let info = s
        .api(
            &install.token,
            "conversations.info",
            json!({"channel":q.channel_id}),
        )
        .await?;
    let channel = &info["channel"];
    if channel["is_archived"] == true || channel["is_im"] == true || channel["is_mpim"] == true {
        return Err(ApiError::forbidden("slack_channel_access_unavailable"));
    }
    let public = channel["is_private"] != true;
    let channel_name = channel["name"]
        .as_str()
        .filter(|n| !n.is_empty() && n.len() <= KEY_LIMIT)
        .unwrap_or(&q.channel_id)
        .to_owned();
    // Thread replies only arrive as events in channels the bot belongs to.
    // Private channels need an invitation; public ones can be joined.
    if channel["is_member"] != true {
        if !public {
            return Err(ApiError::forbidden("slack_channel_access_unavailable"));
        }
        s.api(
            &install.token,
            "conversations.join",
            json!({"channel":q.channel_id}),
        )
        .await?;
    }
    let starter = s
        .api(&install.token, "users.info", json!({"user":user}))
        .await
        .ok()
        .and_then(|v| display_name(&v["user"]));
    let text = match starter {
        Some(name) => format!(
            "{} · Started from Zuse by {}",
            escape(&title),
            escape(&name)
        ),
        None => format!("{} · Started from Zuse", escape(&title)),
    };
    let mut body = json!({"channel":q.channel_id,"text":text,"mrkdwn":false,"unfurl_links":false,"unfurl_media":false});
    s.persona_fields(q.persona.as_ref(), &mut body);
    let posted = s.post_message(&install.token, body, public).await?;
    let thread_ts = posted["ts"]
        .as_str()
        .filter(|ts| !ts.is_empty() && ts.len() <= KEY_LIMIT)
        .ok_or_else(unavailable)?
        .to_owned();
    let inserted = sqlx::query("INSERT INTO zuse_rust_slack_links(link_id,account_id,actor_id,chat_key,team_id,channel_id,channel_name,thread_ts,sync,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,true,$9) ON CONFLICT DO NOTHING")
        .bind(nonce()).bind(&access.owner_id).bind(&access.actor.account_id).bind(&q.chat_key)
        .bind(&q.team_id).bind(&q.channel_id).bind(&channel_name).bind(&thread_ts).bind(now())
        .execute(&s.pool).await?.rows_affected();
    if inserted == 0 {
        // A concurrent request linked this chat first; withdraw our parent message.
        let _ = s
            .api(
                &install.token,
                "chat.delete",
                json!({"channel":q.channel_id,"ts":thread_ts}),
            )
            .await;
    }
    let row = s
        .link_row(&access, &q.chat_key)
        .await?
        .ok_or_else(unavailable)?;
    if row.team_id != q.team_id || row.channel_id != q.channel_id {
        return Err(ApiError::conflict("slack_chat_already_linked"));
    }
    let permalink = s.permalink(&row).await;
    Ok(Json(json!({"link":row.view(permalink)})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Update {
    chat_key: String,
    sync: bool,
}
pub(super) async fn update(
    State(s): State<Slack>,
    headers: HeaderMap,
    Json(q): Json<Update>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    if !key_ok(&q.chat_key) {
        return Err(invalid());
    }
    let updated = sqlx::query("UPDATE zuse_rust_slack_links SET sync=$4 WHERE account_id=$1 AND actor_id=$2 AND chat_key=$3")
        .bind(&access.owner_id).bind(&access.actor.account_id).bind(&q.chat_key).bind(q.sync)
        .execute(&s.pool).await?.rows_affected();
    let row = s.link_row(&access, &q.chat_key).await?;
    let row = row
        .filter(|_| updated == 1)
        .ok_or_else(|| ApiError::not_found("slack_chat_link_not_found"))?;
    let permalink = s.permalink(&row).await;
    Ok(Json(json!({"link":row.view(permalink)})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Remove {
    chat_key: String,
}
pub(super) async fn remove(
    State(s): State<Slack>,
    headers: HeaderMap,
    Json(q): Json<Remove>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    if !key_ok(&q.chat_key) {
        return Err(invalid());
    }
    // Mirrored-reply receipts and queued replies cascade with the link.
    let removed = sqlx::query(
        "DELETE FROM zuse_rust_slack_links WHERE account_id=$1 AND actor_id=$2 AND chat_key=$3",
    )
    .bind(&access.owner_id)
    .bind(&access.actor.account_id)
    .bind(&q.chat_key)
    .execute(&s.pool)
    .await?
    .rows_affected();
    if removed == 0 {
        return Err(ApiError::not_found("slack_chat_link_not_found"));
    }
    Ok(Json(json!({"removed":true})))
}

pub(super) async fn post(
    State(s): State<Slack>,
    headers: HeaderMap,
    Json(q): Json<PostRequest>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    if !key_ok(&q.chat_key) || !key_ok(&q.message_id) {
        return Err(invalid());
    }
    if q.persona.as_ref().is_some_and(|p| !p.is_valid()) {
        return Err(ApiError::bad_request("slack_persona_invalid"));
    }
    let text = mirror_text(q.text.trim());
    if text.trim().is_empty() {
        return Err(ApiError::bad_request("slack_post_empty"));
    }
    let row = s
        .link_row(&access, &q.chat_key)
        .await?
        .ok_or_else(|| ApiError::not_found("slack_chat_link_not_found"))?;
    if !row.sync {
        return Ok(Json(json!({"posted":false,"ts":null})));
    }
    // Claim the message before posting so retries return the first post.
    let existing: Option<(Option<String>, i64)> = sqlx::query_as(
        "SELECT ts,created_at FROM zuse_rust_slack_link_posts WHERE link_id=$1 AND message_id=$2",
    )
    .bind(&row.link_id)
    .bind(&q.message_id)
    .fetch_optional(&s.pool)
    .await?;
    match existing {
        Some((Some(ts), _)) => return Ok(Json(json!({"posted":true,"ts":ts}))),
        Some((None, created)) if now() - created < POST_CLAIM_MS => {
            return Err(ApiError::conflict("slack_post_in_progress"));
        }
        // A claim lost to a crash before posting: take it over.
        Some((None, created)) => {
            sqlx::query("DELETE FROM zuse_rust_slack_link_posts WHERE link_id=$1 AND message_id=$2 AND ts IS NULL AND created_at=$3")
                .bind(&row.link_id).bind(&q.message_id).bind(created)
                .execute(&s.pool).await?;
        }
        None => {}
    }
    let claimed = sqlx::query("INSERT INTO zuse_rust_slack_link_posts(link_id,message_id,created_at) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
        .bind(&row.link_id).bind(&q.message_id).bind(now())
        .execute(&s.pool).await?.rows_affected();
    if claimed != 1 {
        return Err(ApiError::conflict("slack_post_in_progress"));
    }
    let result = async {
        let install = s.install(&row.team_id).await?;
        let mut body = json!({"channel":row.channel_id,"thread_ts":row.thread_ts,"text":escape(&text),"mrkdwn":true,"unfurl_links":false,"unfurl_media":false});
        s.persona_fields(q.persona.as_ref(), &mut body);
        let posted = s.post_message(&install.token, body, false).await?;
        posted["ts"]
            .as_str()
            .filter(|ts| !ts.is_empty() && ts.len() <= KEY_LIMIT)
            .map(str::to_owned)
            .ok_or_else(unavailable)
    }
    .await;
    match result {
        Ok(ts) => {
            sqlx::query(
                "UPDATE zuse_rust_slack_link_posts SET ts=$3 WHERE link_id=$1 AND message_id=$2",
            )
            .bind(&row.link_id)
            .bind(&q.message_id)
            .bind(&ts)
            .execute(&s.pool)
            .await?;
            Ok(Json(json!({"posted":true,"ts":ts})))
        }
        Err(error) => {
            sqlx::query("DELETE FROM zuse_rust_slack_link_posts WHERE link_id=$1 AND message_id=$2 AND ts IS NULL")
                .bind(&row.link_id).bind(&q.message_id)
                .execute(&s.pool).await?;
            Err(error)
        }
    }
}

pub(super) async fn inbox(
    State(s): State<Slack>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    let rows = sqlx::query("SELECT q.sequence,q.link_id,q.event_id,q.message_ts,q.payload,l.chat_key,l.team_id,l.channel_name FROM zuse_rust_slack_inbox q JOIN zuse_rust_slack_links l USING(link_id) WHERE l.account_id=$1 AND l.actor_id=$2 AND l.sync ORDER BY q.sequence LIMIT $3")
        .bind(&access.owner_id).bind(&access.actor.account_id).bind(INBOX_PAGE)
        .fetch_all(&s.pool).await?;
    let mut tokens: HashMap<String, Option<String>> = HashMap::new();
    let mut names: HashMap<(String, String), Option<String>> = HashMap::new();
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        let team: String = row.get("team_id");
        let queued: Queued = s.open(
            &inbox_context(
                &row.get::<String, _>("link_id"),
                &row.get::<String, _>("message_ts"),
            ),
            &row.get::<String, _>("payload"),
        )?;
        // Names are cosmetic: a lookup failure leaves the author unnamed.
        let key = (team.clone(), queued.user.clone());
        if !names.contains_key(&key) {
            if !tokens.contains_key(&team) {
                let token = s.install(&team).await.ok().map(|i| i.token);
                tokens.insert(team.clone(), token);
            }
            let name = match tokens.get(&team).cloned().flatten() {
                Some(token) => s
                    .api(&token, "users.info", json!({"user":queued.user}))
                    .await
                    .ok()
                    .and_then(|v| display_name(&v["user"])),
                None => None,
            };
            names.insert(key.clone(), name);
        }
        messages.push(zuse_protocol::slack::InboxMessage {
            sequence: row.get("sequence"),
            chat_key: row.get("chat_key"),
            event_id: row.get("event_id"),
            text: queued.text,
            author: names.get(&key).cloned().flatten(),
            channel: Some(row.get("channel_name")),
        });
    }
    Ok(Json(json!({ "messages": messages })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Ack {
    sequences: Vec<i64>,
}
pub(super) async fn ack(
    State(s): State<Slack>,
    headers: HeaderMap,
    Json(q): Json<Ack>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    if q.sequences.len() > ACK_LIMIT {
        return Err(invalid());
    }
    if q.sequences.is_empty() {
        return Ok(Json(json!({"acked":0})));
    }
    let acked = sqlx::query("DELETE FROM zuse_rust_slack_inbox WHERE sequence=ANY($1) AND link_id IN (SELECT link_id FROM zuse_rust_slack_links WHERE account_id=$2 AND actor_id=$3)")
        .bind(&q.sequences).bind(&access.owner_id).bind(&access.actor.account_id)
        .execute(&s.pool).await?.rows_affected();
    Ok(Json(json!({ "acked": acked })))
}

/// Side of the rendered avatar; Slack scales icons down from here.
pub(super) const AVATAR_SIZE: u32 = 512;
const AVATAR_BACKGROUND: [u8; 3] = [0xF4, 0xF1, 0xEA];
/// Render the current round-face mascot definitions without a window or GPU.
pub(super) fn render_avatar(avatar: &ZapperAvatar) -> Result<Vec<u8>, ApiError> {
    use zuse_protocol::presentation::zapper_mascots::{self, Face};
    let (mascot, color) = zapper_mascots::resolve(avatar);
    let size = AVATAR_SIZE as usize;
    let mut pixels = AVATAR_BACKGROUND.repeat(size * size);
    for py in 0..size {
        for px in 0..size {
            let x = (px as f32 + 0.5) / size as f32;
            let y = (py as f32 + 0.5) / size as f32;
            let disc = |cx: f32, cy: f32, r: f32| (x - cx).hypot(y - cy) <= r;
            let pill = |left: f32, top: f32, w: f32, h: f32, r: f32| {
                let dx = ((x - (left + w / 2.)).abs() - (w / 2. - r)).max(0.);
                let dy = ((y - (top + h / 2.)).abs() - (h / 2. - r)).max(0.);
                dx.hypot(dy) <= r
            };
            let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32, width: f32| {
                let dx = x - cx;
                let dy = y - cy;
                let angle = dy.atan2(dx).to_degrees().rem_euclid(360.);
                (dx.hypot(dy) - r).abs() <= width / 2. && angle >= from && angle <= to
            };
            if !disc(0.5, 0.5, 0.5) {
                continue;
            }
            let ink = match mascot.face {
                Face::Smile => {
                    disc(0.37, 0.44, 0.072)
                        || disc(0.63, 0.44, 0.072)
                        || arc(0.5, 0.53, 0.14, 25., 155., 0.065)
                }
                Face::Tall => {
                    pill(0.325, 0.36, 0.11, 0.22, 0.055) || pill(0.565, 0.36, 0.11, 0.22, 0.055)
                }
                Face::Visor => {
                    pill(0.2, 0.37, 0.6, 0.19, 0.095)
                        && !disc(0.37, 0.465, 0.042)
                        && !disc(0.63, 0.465, 0.042)
                }
                Face::Cyclops => disc(0.5, 0.46, 0.19) && !disc(0.555, 0.405, 0.06),
                Face::Happy => {
                    arc(0.36, 0.49, 0.085, 200., 340., 0.07)
                        || arc(0.64, 0.49, 0.085, 200., 340., 0.07)
                        || arc(0.5, 0.57, 0.08, 25., 155., 0.06)
                }
                Face::Shades => {
                    pill(0.235, 0.38, 0.235, 0.15, 0.05)
                        || pill(0.53, 0.38, 0.235, 0.15, 0.05)
                        || pill(0.44, 0.405, 0.12, 0.04, 0.02)
                        || pill(0.42, 0.65, 0.16, 0.045, 0.0225)
                }
                Face::Wink => {
                    disc(0.37, 0.44, 0.072)
                        || pill(0.55, 0.425, 0.16, 0.05, 0.025)
                        || arc(0.53, 0.53, 0.12, 30., 120., 0.065)
                }
                Face::Sleepy => {
                    arc(0.36, 0.42, 0.085, 20., 160., 0.065)
                        || arc(0.64, 0.42, 0.085, 20., 160., 0.065)
                        || disc(0.5, 0.65, 0.05)
                }
                Face::Blush => {
                    disc(0.37, 0.45, 0.068) || disc(0.63, 0.45, 0.068) || disc(0.5, 0.6, 0.042)
                }
            };
            let rgb = if ink { 0x161616 } else { color.body };
            let mut rgb = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
            if !ink
                && mascot.face == Face::Blush
                && (disc(0.27, 0.58, 0.075) || disc(0.73, 0.58, 0.075))
            {
                let (cheek, alpha) = if color.body == 0xff8fbf {
                    (color.shade, 1.)
                } else {
                    (0xff6b8b, 0.55)
                };
                for (channel, shift) in rgb.iter_mut().zip([16, 8, 0]) {
                    *channel = (*channel as f32 * (1. - alpha)
                        + ((cheek >> shift) & 255) as f32 * alpha)
                        .round() as u8;
                }
            }
            let at = (py * size + px) * 3;
            pixels[at..at + 3].copy_from_slice(&rgb);
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, AVATAR_SIZE, AVATAR_SIZE);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|_| unavailable())?;
    writer
        .write_image_data(&pixels)
        .map_err(|_| unavailable())?;
    writer.finish().map_err(|_| unavailable())?;
    Ok(out)
}
pub(super) async fn avatar(Path((mascot, file)): Path<(String, String)>) -> Response {
    let color = file.strip_suffix(".png").unwrap_or("");
    if !avatar_id_ok(&mascot) || !avatar_id_ok(color) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match render_avatar(&ZapperAvatar {
        mascot,
        color: color.to_owned(),
    }) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=86400"),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => error.into_response(),
    }
}
