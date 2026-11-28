//! Pull request work the cloud does itself: merge-conflict checks after a
//! push, and delivering events to chats hosted on cloud computers.
//!
//! Webhooks wake the worker at once; a short timer covers checks that come
//! due later. GitHub is only asked about a pull request after a push that can
//! change whether it merges cleanly, never on a schedule of its own.
use super::*;
use crate::access::Scope;
use crate::auth::Principal;
use std::collections::{BTreeMap, HashMap};
use zuse_protocol::cloud_task_commands as wire;
use zuse_protocol::pull_request_links::{WAKE_INSTRUCTION, activity_text};

/// Woken after every webhook that recorded something.
pub(crate) static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// Checks GitHub may still be computing are retried this many times.
const CONFLICT_ATTEMPTS: i32 = 4;
/// Comments and reviews arriving together wait until none has come for this long.
const FEEDBACK_QUIET_MS: i64 = 20_000;
const BATCH: i64 = 20;

pub fn wake() {
    WAKE.notify_one();
}

#[derive(Clone)]
pub struct PullRequestWorker {
    github: Arc<NativeGithub>,
    runtime: Option<Arc<crate::runtime::RuntimeControl>>,
    identity: Arc<dyn crate::auth::Identity>,
    organizations_enabled: bool,
    organization_ids: Arc<Vec<String>>,
}

impl PullRequestWorker {
    pub fn new(app: &crate::routes::AppState) -> Option<Self> {
        Some(Self {
            github: app.store.github_native.clone()?,
            runtime: app.runtime.clone(),
            identity: app.identity.clone(),
            organizations_enabled: app.organizations_enabled,
            organization_ids: app.organization_ids.clone(),
        })
    }

    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match self.work_once().await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!(code = error.code, "pull request work deferred")
                    }
                }
                tokio::select! {
                    _ = WAKE.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                }
            }
        })
    }

    /// A worker with no runtime (no cloud-chat delivery) and no organizations.
    #[cfg(test)]
    pub(super) fn for_test(github: Arc<NativeGithub>) -> Self {
        struct NoIdentity;
        #[async_trait::async_trait]
        impl crate::auth::Identity for NoIdentity {
            async fn verify(&self, _: &str) -> Result<Principal, ApiError> {
                Err(ApiError::unauthorized("test_bearer_unused"))
            }
            async fn membership(
                &self,
                _: &str,
                _: &str,
            ) -> Result<crate::auth::Membership, ApiError> {
                Err(ApiError::forbidden("test_membership_unused"))
            }
        }
        Self {
            github,
            runtime: None,
            identity: Arc::new(NoIdentity),
            organizations_enabled: false,
            organization_ids: Arc::new(vec![]),
        }
    }

    /// One pass; true when there may be more to do right away.
    pub async fn work_once(&self) -> Result<bool, ApiError> {
        if !self.github.pull_request_schema().await? {
            return Ok(false);
        }
        let conflicts = self.check_conflicts().await?;
        let delivered = self.deliver_hosted().await?;
        Ok(conflicts || delivered)
    }

    /// Ask GitHub whether each due pull request still merges cleanly.
    pub(super) async fn check_conflicts(&self) -> Result<bool, ApiError> {
        let mut tx = self.github.pool.begin().await?;
        let rows = sqlx::query("SELECT account_id,repository_id,number,installation_id,repository,base_ref,conflict_attempts,conflict_sha,policy FROM zuse_rust_pull_request_owners WHERE conflict_due_at<=$1 AND state='open' ORDER BY conflict_due_at LIMIT $2 FOR UPDATE SKIP LOCKED")
            .bind(now()).bind(BATCH)
            .fetch_all(&mut *tx).await?;
        let full = rows.len() as i64 == BATCH;
        let mut tokens: HashMap<(i64, String), Option<Zeroizing<String>>> = HashMap::new();
        for row in rows {
            let account: String = row.try_get("account_id")?;
            let repository_id: i64 = row.try_get("repository_id")?;
            let number: i64 = row.try_get("number")?;
            let installation: i64 = row.try_get("installation_id")?;
            let repository: String = row.try_get("repository")?;
            let attempts: i32 = row.try_get("conflict_attempts")?;
            let key = (installation, repository.clone());
            if !tokens.contains_key(&key) {
                let token = self
                    .github
                    .installation_token(installation, &repository)
                    .await
                    .ok();
                tokens.insert(key.clone(), token);
            }
            let pr = match tokens.get(&key).cloned().flatten() {
                Some(token) => self
                    .github
                    .get(&format!("/repos/{repository}/pulls/{number}"), &token)
                    .await
                    .ok(),
                None => None,
            };
            let mergeable = pr.as_ref().map(|pr| pr["mergeable"].as_bool());
            // GitHub computes mergeability in the background: ask again shortly.
            if matches!(mergeable, None | Some(None)) && attempts + 1 < CONFLICT_ATTEMPTS {
                sqlx::query("UPDATE zuse_rust_pull_request_owners SET conflict_due_at=$4,conflict_attempts=conflict_attempts+1 WHERE account_id=$1 AND repository_id=$2 AND number=$3")
                    .bind(&account).bind(repository_id).bind(number).bind(now() + CONFLICT_DELAY_MS)
                    .execute(&mut *tx).await?;
                continue;
            }
            sqlx::query("UPDATE zuse_rust_pull_request_owners SET conflict_due_at=NULL,conflict_attempts=0 WHERE account_id=$1 AND repository_id=$2 AND number=$3")
                .bind(&account).bind(repository_id).bind(number)
                .execute(&mut *tx).await?;
            let Some(pr) = pr else { continue };
            match mergeable.flatten() {
                Some(false) => {
                    let pair = format!(
                        "{}:{}",
                        pr["head"]["sha"].as_str().unwrap_or(""),
                        pr["base"]["sha"].as_str().unwrap_or("")
                    );
                    let previous: Option<String> = row.try_get("conflict_sha")?;
                    if previous.as_deref() == Some(pair.as_str()) {
                        continue;
                    }
                    sqlx::query("UPDATE zuse_rust_pull_request_owners SET conflict_sha=$4 WHERE account_id=$1 AND repository_id=$2 AND number=$3")
                        .bind(&account).bind(repository_id).bind(number).bind(&pair)
                        .execute(&mut *tx).await?;
                    let policy: EventPolicy =
                        serde_json::from_value(row.try_get("policy")?).unwrap_or_default();
                    let action = policy.action(PullRequestEventKind::Conflict, false);
                    if action == EventAction::Ignore {
                        continue;
                    }
                    let base = row
                        .try_get::<Option<String>, _>("base_ref")?
                        .unwrap_or_else(|| "its base branch".into());
                    let draft = Draft {
                        kind: PullRequestEventKind::Conflict,
                        event_id: format!("conflict:{}", hash(&pair)),
                        url: pr["html_url"].as_str().unwrap_or("").to_owned(),
                        author: None,
                        author_id: None,
                        bot: false,
                        body: format!(
                            "It has merge conflicts with {base}. Merge or rebase onto {base}, resolve them, and push."
                        ),
                        location: None,
                    };
                    let event_id = format!("{repository_id}:{number}:{}", draft.event_id);
                    self.github
                        .record(
                            &mut tx,
                            &account,
                            repository_id,
                            number,
                            &event_id,
                            &draft,
                            action,
                        )
                        .await?;
                }
                Some(true) => {
                    sqlx::query("UPDATE zuse_rust_pull_request_owners SET conflict_sha=NULL WHERE account_id=$1 AND repository_id=$2 AND number=$3")
                        .bind(&account).bind(repository_id).bind(number)
                        .execute(&mut *tx).await?;
                }
                None => {}
            }
        }
        tx.commit().await?;
        Ok(full)
    }

    /// Queue events for chats on cloud computers into their conversations.
    async fn deliver_hosted(&self) -> Result<bool, ApiError> {
        let Some(runtime) = self.runtime.clone() else {
            return Ok(false);
        };
        let mut tx = self.github.pool.begin().await?;
        sqlx::query("DELETE FROM zuse_rust_pull_request_inbox q USING zuse_rust_pull_request_owners o WHERE o.account_id=q.account_id AND o.repository_id=q.repository_id AND o.number=q.number AND o.workspace_id IS NOT NULL AND q.created_at<$1")
            .bind(now() - INBOX_RETENTION_MS)
            .execute(&mut *tx).await?;
        let rows = sqlx::query("SELECT q.sequence,q.event_id,q.payload,q.created_at,o.account_id,o.actor_id,o.chat_key,o.workspace_id,o.repository,o.number FROM zuse_rust_pull_request_inbox q JOIN zuse_rust_pull_request_owners o USING(account_id,repository_id,number) WHERE o.workspace_id IS NOT NULL ORDER BY q.sequence LIMIT 100 FOR UPDATE OF q SKIP LOCKED")
            .fetch_all(&mut *tx).await?;
        let mut groups: BTreeMap<(String, String, String, String), Vec<InboxEvent>> =
            BTreeMap::new();
        for row in &rows {
            let account: String = row.try_get("account_id")?;
            let event_id: String = row.try_get("event_id")?;
            let sealed: Sealed = self.github.open(
                &inbox_context(&account, &event_id),
                &row.try_get::<String, _>("payload")?,
            )?;
            let key = (
                account,
                row.try_get("actor_id")?,
                row.try_get::<String, _>("workspace_id")?,
                row.try_get("chat_key")?,
            );
            groups.entry(key.clone()).or_default().push(InboxEvent {
                sequence: row.try_get("sequence")?,
                chat_key: key.3.clone(),
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
        let mut handled = Vec::new();
        let at = now();
        for ((account, actor, workspace, chat_key), events) in groups {
            let quiet = events
                .iter()
                .filter(|e| e.kind.is_feedback())
                .map(|e| e.created_at)
                .max()
                .is_none_or(|latest| at - latest >= FEEDBACK_QUIET_MS);
            // Context waits for the next event that starts a turn.
            if !quiet || !events.iter().any(|e| e.action == EventAction::Wake) {
                continue;
            }
            let Some(session) = chat_key.strip_prefix("chat:") else {
                continue;
            };
            let ids: Vec<&str> = events.iter().map(|e| e.event_id.as_str()).collect();
            let command = format!(
                "github-{}",
                hash(&format!("{chat_key}\n{}", ids.join("\n")))
            );
            let request = wire::Request {
                command_id: command.clone(),
                session_id: session.to_owned(),
                action: wire::Action::Queue {
                    queue_id: command.clone(),
                    message_id: format!("{command}-message"),
                    input: wire::Input {
                        run_selection: None,
                        text: format!("{}\n\n{WAKE_INSTRUCTION}", activity_text(&events)),
                        attachments: vec![],
                        computer_files: vec![],
                        file_refs: vec![],
                        skill_refs: vec![],
                        annotations: vec![],
                        as_goal: false,
                    },
                    hold_for_turn_end: false,
                },
                wake: wire::WakePolicy::ExplicitResume,
            };
            let result = match self.access(&account, &actor).await {
                Ok(access) => runtime
                    .task_command_submit(&workspace, &access, request, at)
                    .await
                    .map(|_| ()),
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => handled.extend(events.iter().map(|e| e.sequence)),
                // The chat or its access is gone: these events can never land.
                Err(error) if matches!(error.status.as_u16(), 400 | 403 | 404) => {
                    tracing::warn!(
                        code = error.code,
                        "pull request activity dropped for a cloud chat"
                    );
                    handled.extend(events.iter().map(|e| e.sequence));
                }
                Err(error) => {
                    tracing::debug!(
                        code = error.code,
                        "pull request activity for a cloud chat deferred"
                    )
                }
            }
        }
        if !handled.is_empty() {
            sqlx::query("DELETE FROM zuse_rust_pull_request_inbox WHERE sequence=ANY($1)")
                .bind(&handled)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(false)
    }

    /// Act as the member who owns the chat, checked freshly.
    async fn access(&self, account: &str, actor: &str) -> Result<Access, ApiError> {
        if account == actor {
            return Ok(Access {
                actor: Principal {
                    account_id: actor.to_owned(),
                },
                scope: Scope::Personal,
                owner_id: account.to_owned(),
                membership: None,
            });
        }
        let organization = account
            .strip_prefix("organization:")
            .filter(|id| {
                self.organizations_enabled && self.organization_ids.iter().any(|o| o == id)
            })
            .ok_or_else(|| ApiError::forbidden("workspace_access_denied"))?;
        let membership = self.identity.membership(actor, organization).await?;
        if membership.status != "active"
            || membership.user_id != actor
            || !matches!(membership.role.slug.as_str(), "member" | "admin")
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        Ok(Access {
            actor: Principal {
                account_id: actor.to_owned(),
            },
            scope: Scope::Organization {
                organization_id: organization.to_owned(),
            },
            owner_id: account.to_owned(),
            membership: Some(membership),
        })
    }
}

impl NativeGithub {
    /// A short-lived installation token limited to reading one repository's
    /// pull requests.
    pub(super) async fn installation_token(
        &self,
        installation: i64,
        repository: &str,
    ) -> Result<Zeroizing<String>, ApiError> {
        let name = repository
            .split_once('/')
            .map(|(_, name)| name)
            .filter(|name| valid_component(name))
            .ok_or_else(invalid)?;
        let (status, value) = self
            .response(
                self.http
                    .post(self.api(&format!("/app/installations/{installation}/access_tokens")))
                    .bearer_auth(self.app_token()?)
                    .json(&json!({"repositories":[name],"permissions":{"pull_requests":"read"}})),
            )
            .await?;
        if status != 201 {
            return Err(ApiError::forbidden("github_installation_token_failed"));
        }
        value["token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(|t| Zeroizing::new(t.to_owned()))
            .ok_or_else(unavailable)
    }
}
