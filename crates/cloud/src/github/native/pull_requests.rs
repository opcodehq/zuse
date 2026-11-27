//! Pull requests owned by chats, and webhook routing of their events.
//!
//! A claim is checked with the actor's own repository grant, the same
//! authority Git pushes use. Webhook routing reads only the database: an
//! event reaches a chat only through an owner row whose installation and
//! repository match the delivery.
use super::*;
use events::{Classified, Draft, Target};
use zuse_protocol::pull_request_links::{
    ClaimRequest, ClaimSource, EventAction, EventPolicy, InboxEvent, OwnedPullRequest,
    PullRequestEventKind, valid_chat_key, valid_repository,
};

pub(crate) mod events;

/// Events returned per inbox read.
const INBOX_PAGE: i64 = 50;
const ACK_LIMIT: usize = 200;
/// Undelivered events older than this are dropped.
const INBOX_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Owned pull requests listed per account member.
const LIST_LIMIT: i64 = 500;

/// What the inbox stores, sealed. The owner row supplies the chat.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sealed {
    kind: PullRequestEventKind,
    action: EventAction,
    url: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    location: Option<String>,
}

fn inbox_context(account: &str, event_id: &str) -> String {
    format!("pull-request-inbox:{account}:{event_id}")
}

const OWNER_COLUMNS: &str = "chat_key,repository,number,url,title,state,draft,head_ref,claimed_by,claimed_at,checks,policy";

fn owned(row: &sqlx::postgres::PgRow) -> Result<OwnedPullRequest, ApiError> {
    Ok(OwnedPullRequest {
        chat_key: row.try_get("chat_key")?,
        repository: row.try_get("repository")?,
        number: row.try_get::<i64, _>("number")? as u64,
        url: row.try_get("url")?,
        title: row.try_get("title")?,
        state: row.try_get("state")?,
        draft: row.try_get("draft")?,
        head_ref: row.try_get("head_ref")?,
        claimed_by: ClaimSource::parse(&row.try_get::<String, _>("claimed_by")?)
            .ok_or_else(unavailable)?,
        claimed_at: row.try_get("claimed_at")?,
        checks: row.try_get("checks")?,
        policy: serde_json::from_value(row.try_get("policy")?).unwrap_or_default(),
    })
}

fn invalid() -> ApiError {
    ApiError::bad_request("invalid_pull_request_link")
}

fn not_found() -> ApiError {
    ApiError::not_found("pull_request_not_owned")
}

/// Decide what the owning chat does with an event.
///
/// Comments the account's own GitHub user wrote are usually the agent
/// speaking through its token, so they only become context unless they call
/// on Zuse by name.
pub(crate) fn decide(policy: &EventPolicy, draft: &Draft, own_user: Option<i64>) -> EventAction {
    let action = policy.action(draft.kind, draft.bot);
    let own = draft.author_id.is_some() && draft.author_id == own_user;
    if action == EventAction::Wake
        && own
        && draft.kind.is_feedback()
        && !events::mentions_zuse(&draft.body)
    {
        return EventAction::Context;
    }
    action
}

impl NativeGithub {
    async fn pull_request_schema(&self) -> Result<bool, ApiError> {
        Ok(sqlx::query_scalar(
            "SELECT to_regclass('zuse_rust_pull_request_owners') IS NOT NULL AND to_regclass('zuse_rust_pull_request_inbox') IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    async fn require_pull_requests(&self, access: &Access) -> Result<(), ApiError> {
        self.require(access)?;
        if !self.pull_request_schema().await? {
            return Err(ApiError::unavailable("pull_request_links_unavailable"));
        }
        Ok(())
    }

    /// Make `chat_key` the owner of a pull request. A branch claim never takes
    /// a pull request another chat owns; it returns the current owner instead.
    pub async fn claim_pull_request(
        &self,
        access: &Access,
        request: &ClaimRequest,
    ) -> Result<OwnedPullRequest, ApiError> {
        self.require_pull_requests(access).await?;
        if !valid_chat_key(&request.chat_key)
            || !valid_repository(&request.repository)
            || request.number.is_some() == request.branch.is_some()
            || request.number == Some(0)
            || request
                .branch
                .as_deref()
                .is_some_and(|b| !crate::runtime::creation::valid_git_ref(b))
        {
            return Err(invalid());
        }
        let grant = self
            .scoped_git(&access.owner_id, &access.actor.account_id, &request.repository)
            .await?;
        let pr = match (request.number, &request.branch) {
            (Some(number), _) => {
                self.get(
                    &format!("/repos/{}/pulls/{number}", request.repository),
                    &grant.token,
                )
                .await?
            }
            (None, Some(branch)) => {
                let owner = request.repository.split_once('/').map(|(o, _)| o).unwrap_or("");
                let mut url = self.api(&format!("/repos/{}/pulls", request.repository));
                url.query_pairs_mut()
                    .append_pair("head", &format!("{owner}:{branch}"))
                    .append_pair("state", "open")
                    .append_pair("per_page", "1");
                let (status, value) = self
                    .response(self.http.get(url).bearer_auth(grant.token.as_str()))
                    .await?;
                if status != 200 {
                    return Err(ApiError::forbidden("github_repository_access_denied"));
                }
                value
                    .as_array()
                    .and_then(|list| list.first())
                    .cloned()
                    .ok_or_else(|| ApiError::not_found("pull_request_not_found"))?
            }
            (None, None) => return Err(invalid()),
        };
        self.store_claim(access, request, &pr, grant.installation_id)
            .await
    }

    /// Record a claim for pull request `pr` (a REST pull request object).
    pub(super) async fn store_claim(
        &self,
        access: &Access,
        request: &ClaimRequest,
        pr: &Value,
        installation_id: i64,
    ) -> Result<OwnedPullRequest, ApiError> {
        let number = pr["number"].as_i64().filter(|n| *n > 0).ok_or_else(unavailable)?;
        let repository_id = pr["base"]["repo"]["id"]
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or_else(unavailable)?;
        let repository = pr["base"]["repo"]["full_name"]
            .as_str()
            .filter(|r| valid_repository(r))
            .unwrap_or(&request.repository)
            .to_owned();
        let url = pr["html_url"]
            .as_str()
            .filter(|u| u.starts_with("https://github.com/") && u.len() <= 512)
            .ok_or_else(unavailable)?;
        let title = events::clip(pr["title"].as_str().unwrap_or(""), 500);
        let head_ref = pr["head"]["ref"].as_str().filter(|r| r.len() <= 255);
        let head_sha = pr["head"]["sha"]
            .as_str()
            .filter(|s| s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        let at = now();
        // Ownership moves unless this is a branch claim on someone else's pull
        // request; the copy of the pull request is refreshed either way.
        let row = sqlx::query(&format!(
            "INSERT INTO zuse_rust_pull_request_owners(account_id,repository_id,number,installation_id,repository,actor_id,chat_key,claimed_by,claimed_at,url,title,state,draft,head_ref,head_sha,updated_at)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$9)
             ON CONFLICT(account_id,repository_id,number) DO UPDATE SET
              installation_id=EXCLUDED.installation_id,repository=EXCLUDED.repository,url=EXCLUDED.url,title=EXCLUDED.title,
              state=EXCLUDED.state,draft=EXCLUDED.draft,head_ref=EXCLUDED.head_ref,head_sha=EXCLUDED.head_sha,updated_at=EXCLUDED.updated_at,
              actor_id=CASE WHEN $16 THEN EXCLUDED.actor_id ELSE zuse_rust_pull_request_owners.actor_id END,
              chat_key=CASE WHEN $16 THEN EXCLUDED.chat_key ELSE zuse_rust_pull_request_owners.chat_key END,
              claimed_by=CASE WHEN $16 THEN EXCLUDED.claimed_by ELSE zuse_rust_pull_request_owners.claimed_by END,
              claimed_at=CASE WHEN $16 THEN EXCLUDED.claimed_at ELSE zuse_rust_pull_request_owners.claimed_at END
             RETURNING {OWNER_COLUMNS}"
        ))
        .bind(&access.owner_id)
        .bind(repository_id)
        .bind(number)
        .bind(installation_id)
        .bind(&repository)
        .bind(&access.actor.account_id)
        .bind(&request.chat_key)
        .bind(request.source.as_str())
        .bind(at)
        .bind(url)
        .bind(&title)
        .bind(events::state(pr))
        .bind(pr["draft"] == true)
        .bind(head_ref)
        .bind(head_sha)
        .bind(request.source.takes_over())
        .fetch_one(&self.pool)
        .await?;
        owned(&row)
    }

    pub async fn release_pull_request(
        &self,
        access: &Access,
        chat_key: &str,
        repository: &str,
        number: u64,
    ) -> Result<(), ApiError> {
        self.require_pull_requests(access).await?;
        if !valid_chat_key(chat_key) || !valid_repository(repository) || number == 0 {
            return Err(invalid());
        }
        // Pending events go with the row.
        let removed = sqlx::query("DELETE FROM zuse_rust_pull_request_owners WHERE account_id=$1 AND actor_id=$2 AND chat_key=$3 AND lower(repository)=lower($4) AND number=$5")
            .bind(&access.owner_id).bind(&access.actor.account_id).bind(chat_key).bind(repository).bind(number as i64)
            .execute(&self.pool).await?.rows_affected();
        if removed == 0 {
            return Err(not_found());
        }
        Ok(())
    }

    pub async fn set_pull_request_policy(
        &self,
        access: &Access,
        chat_key: &str,
        repository: &str,
        number: u64,
        policy: &EventPolicy,
    ) -> Result<OwnedPullRequest, ApiError> {
        self.require_pull_requests(access).await?;
        if !valid_chat_key(chat_key) || !valid_repository(repository) || number == 0 || !policy.is_valid() {
            return Err(invalid());
        }
        let policy = serde_json::to_value(policy).map_err(|_| invalid())?;
        let row = sqlx::query(&format!("UPDATE zuse_rust_pull_request_owners SET policy=$6,updated_at=$7 WHERE account_id=$1 AND actor_id=$2 AND chat_key=$3 AND lower(repository)=lower($4) AND number=$5 RETURNING {OWNER_COLUMNS}"))
            .bind(&access.owner_id).bind(&access.actor.account_id).bind(chat_key).bind(repository).bind(number as i64).bind(policy).bind(now())
            .fetch_optional(&self.pool).await?.ok_or_else(not_found)?;
        owned(&row)
    }

    /// Every pull request a chat of this member owns.
    pub async fn owned_pull_requests(
        &self,
        access: &Access,
    ) -> Result<Vec<OwnedPullRequest>, ApiError> {
        self.require_pull_requests(access).await?;
        let rows = sqlx::query(&format!("SELECT {OWNER_COLUMNS} FROM zuse_rust_pull_request_owners WHERE account_id=$1 AND actor_id=$2 ORDER BY claimed_at DESC LIMIT $3"))
            .bind(&access.owner_id).bind(&access.actor.account_id).bind(LIST_LIMIT)
            .fetch_all(&self.pool).await?;
        rows.iter().map(owned).collect()
    }

    /// Events waiting for this member's chats, oldest first.
    pub async fn pull_request_inbox(&self, access: &Access) -> Result<Vec<InboxEvent>, ApiError> {
        self.require_pull_requests(access).await?;
        let rows = sqlx::query("SELECT q.sequence,q.event_id,q.payload,q.created_at,o.chat_key,o.repository,o.number FROM zuse_rust_pull_request_inbox q JOIN zuse_rust_pull_request_owners o USING(account_id,repository_id,number) WHERE o.account_id=$1 AND o.actor_id=$2 ORDER BY q.sequence LIMIT $3")
            .bind(&access.owner_id).bind(&access.actor.account_id).bind(INBOX_PAGE)
            .fetch_all(&self.pool).await?;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let event_id: String = row.try_get("event_id")?;
            let sealed: Sealed = self.open(
                &inbox_context(&access.owner_id, &event_id),
                &row.try_get::<String, _>("payload")?,
            )?;
            events.push(InboxEvent {
                sequence: row.try_get("sequence")?,
                chat_key: row.try_get("chat_key")?,
                event_id,
                repository: row.try_get("repository")?,
                number: row.try_get::<i64, _>("number")? as u64,
                kind: sealed.kind,
                action: sealed.action,
                url: sealed.url,
                author: sealed.author,
                body: sealed.body,
                location: sealed.location,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(events)
    }

    pub async fn ack_pull_request_inbox(
        &self,
        access: &Access,
        sequences: &[i64],
    ) -> Result<u64, ApiError> {
        self.require_pull_requests(access).await?;
        if sequences.len() > ACK_LIMIT {
            return Err(invalid());
        }
        let acked = if sequences.is_empty() {
            0
        } else {
            sqlx::query("DELETE FROM zuse_rust_pull_request_inbox q USING zuse_rust_pull_request_owners o WHERE q.sequence=ANY($1) AND o.account_id=q.account_id AND o.repository_id=q.repository_id AND o.number=q.number AND o.account_id=$2 AND o.actor_id=$3")
                .bind(sequences).bind(&access.owner_id).bind(&access.actor.account_id)
                .execute(&self.pool).await?.rows_affected()
        };
        sqlx::query("DELETE FROM zuse_rust_pull_request_inbox WHERE account_id=$1 AND created_at<$2")
            .bind(&access.owner_id)
            .bind(now() - INBOX_RETENTION_MS)
            .execute(&self.pool)
            .await?;
        Ok(acked)
    }

    /// Record a pull request webhook for the chats that own it. Runs inside
    /// the delivery's transaction, so a retried delivery records nothing twice.
    pub(super) async fn route_pull_request_event(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        event: &str,
        value: &Value,
    ) -> Result<(), ApiError> {
        if !events::handles(event) {
            return Ok(());
        }
        let Some(classified) = events::classify(event, value) else {
            return Ok(());
        };
        if !self.pull_request_schema().await? {
            return Ok(());
        }
        let Classified {
            installation_id,
            repository_id,
            target,
            update,
            draft,
        } = classified;
        let (numbers, head_sha) = match &target {
            Target::Number(n) => (vec![*n], None),
            Target::Checks { numbers, head_sha } => (numbers.clone(), Some(head_sha.as_str())),
        };
        // Owner rows of this pull request in every account, joined with each
        // owner's GitHub identity for recognising their own comments.
        let rows = sqlx::query("SELECT o.account_id,o.number,o.policy,o.head_sha,o.checks,o.checks_sha,u.github_user_id FROM zuse_rust_pull_request_owners o LEFT JOIN zuse_rust_github_users u ON u.actor_id=o.actor_id AND u.state='connected' WHERE o.repository_id=$1 AND o.installation_id=$2 AND (o.number=ANY($3) OR ($4::text IS NOT NULL AND o.state='open' AND o.head_sha=$4)) FOR UPDATE OF o")
            .bind(repository_id).bind(installation_id).bind(&numbers).bind(head_sha)
            .fetch_all(&mut **tx).await?;
        for row in rows {
            let account: String = row.try_get("account_id")?;
            let number: i64 = row.try_get("number")?;
            sqlx::query("UPDATE zuse_rust_pull_request_owners SET title=COALESCE($4,title),state=COALESCE($5,state),draft=COALESCE($6,draft),head_ref=COALESCE($7,head_ref),head_sha=COALESCE($8,head_sha),updated_at=$9 WHERE account_id=$1 AND repository_id=$2 AND number=$3")
                .bind(&account).bind(repository_id).bind(number)
                .bind(&update.title).bind(update.state).bind(update.draft).bind(&update.head_ref).bind(&update.head_sha).bind(now())
                .execute(&mut **tx).await?;
            let Some(draft) = &draft else { continue };
            let checks: Option<String> = row.try_get("checks")?;
            let checks_sha: Option<String> = row.try_get("checks_sha")?;
            let owner_head: Option<String> = row.try_get("head_sha")?;
            match draft.kind {
                PullRequestEventKind::ChecksFailed => {
                    sqlx::query("UPDATE zuse_rust_pull_request_owners SET checks='failed',checks_sha=$4 WHERE account_id=$1 AND repository_id=$2 AND number=$3")
                        .bind(&account).bind(repository_id).bind(number).bind(head_sha)
                        .execute(&mut **tx).await?;
                }
                // A pass is only news on the current commit, after a failure on
                // an earlier one.
                PullRequestEventKind::ChecksPassed => {
                    if checks.as_deref() != Some("failed")
                        || checks_sha.as_deref() == head_sha
                        || owner_head.as_deref() != head_sha
                    {
                        continue;
                    }
                    sqlx::query("UPDATE zuse_rust_pull_request_owners SET checks='passed',checks_sha=$4 WHERE account_id=$1 AND repository_id=$2 AND number=$3")
                        .bind(&account).bind(repository_id).bind(number).bind(head_sha)
                        .execute(&mut **tx).await?;
                }
                _ => {}
            }
            let policy: EventPolicy =
                serde_json::from_value(row.try_get("policy")?).unwrap_or_default();
            let action = decide(&policy, draft, row.try_get("github_user_id")?);
            if action == EventAction::Ignore {
                continue;
            }
            let event_id = format!("{}:{number}:{}", repository_id, draft.event_id);
            let payload = self.seal(
                &inbox_context(&account, &event_id),
                &Sealed {
                    kind: draft.kind,
                    action,
                    url: draft.url.clone(),
                    author: draft.author.clone(),
                    body: draft.body.clone(),
                    location: draft.location.clone(),
                },
            )?;
            sqlx::query("INSERT INTO zuse_rust_pull_request_inbox(account_id,repository_id,number,event_id,payload,created_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
                .bind(&account).bind(repository_id).bind(number).bind(&event_id).bind(payload).bind(now())
                .execute(&mut **tx).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod database_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(kind: PullRequestEventKind, author: Option<i64>, bot: bool, body: &str) -> Draft {
        Draft {
            kind,
            event_id: "e".into(),
            url: String::new(),
            author: None,
            author_id: author,
            bot,
            body: body.into(),
            location: None,
        }
    }

    #[test]
    fn own_comments_become_context_unless_they_mention_zuse() {
        let policy = EventPolicy::default();
        let own = draft(PullRequestEventKind::Comment, Some(5), false, "Done, fixed");
        assert_eq!(decide(&policy, &own, Some(5)), EventAction::Context);
        let asked = draft(PullRequestEventKind::Comment, Some(5), false, "@zuse rename it");
        assert_eq!(decide(&policy, &asked, Some(5)), EventAction::Wake);
        let other = draft(PullRequestEventKind::Comment, Some(6), false, "Rename it");
        assert_eq!(decide(&policy, &other, Some(5)), EventAction::Wake);
        // Without a connected identity nothing counts as the owner's own.
        assert_eq!(decide(&policy, &own, None), EventAction::Wake);
    }

    #[test]
    fn bots_and_state_changes_follow_the_policy() {
        let mut policy = EventPolicy::default();
        let bot = draft(PullRequestEventKind::Review, Some(9), true, "LGTM");
        assert_eq!(decide(&policy, &bot, None), EventAction::Ignore);
        policy.bots = Some(EventAction::Wake);
        assert_eq!(decide(&policy, &bot, None), EventAction::Wake);
        let merged = draft(PullRequestEventKind::Merged, Some(5), false, "");
        assert_eq!(decide(&policy, &merged, Some(5)), EventAction::Context);
        let failed = draft(PullRequestEventKind::ChecksFailed, None, false, "test failed");
        assert_eq!(decide(&policy, &failed, Some(5)), EventAction::Wake);
    }
}
