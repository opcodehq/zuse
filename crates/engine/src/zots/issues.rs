//! Issue changes share the Zot receipt transaction and revision check.
use super::*;

pub(super) fn apply(
    zot: &mut Zot,
    command: &ZotCommand,
    tx: &rusqlite::Transaction<'_>,
    actor: Option<&str>,
    coordinator: bool,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut notify = false;
    let mut reply: Option<String> = None;
    let (id, kind, text) = match &command.action {
        ZotAction::CreateIssue { issue_id, details } => {
            valid_id(issue_id)?;
            ensure!(
                !zot.issues.iter().any(|i| i.id == *issue_id),
                "issue already exists"
            );
            validate(zot, issue_id, details)?;
            let number = zot
                .issues
                .iter()
                .map(|i| i.number)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .context("issue numbers exhausted")?;
            zot.issues.push(ZotIssue {
                id: issue_id.clone(),
                number,
                details: details.clone(),
                created_at: now,
                updated_at: now,
                threads: vec![],
                pull_requests: vec![],
                activity: vec![],
            });
            notify = details.assigned && !details.status.terminal();
            (issue_id, "created", "Created the issue".to_owned())
        }
        ZotAction::EditIssue { issue_id, details } => {
            validate(zot, issue_id, details)?;
            let issue = find(zot, issue_id)?;
            if issue.details == *details {
                return Ok(());
            }
            notify = details.assigned && !details.status.terminal();
            let mut changes = vec![];
            if issue.details.status != details.status {
                changes.push(format!("Status changed to {}", details.status.label()));
            }
            if issue.details.priority != details.priority {
                changes.push(format!("Priority changed to {}", details.priority.label()));
            }
            if issue.details.assigned != details.assigned {
                changes.push(
                    if details.assigned {
                        "Assigned to Zapper"
                    } else {
                        "Removed assignee"
                    }
                    .to_owned(),
                );
            }
            if changes.is_empty() {
                changes.push("Updated issue details".into());
            }
            issue.details = details.clone();
            (issue_id, "updated", changes.join(" · "))
        }
        ZotAction::LinkIssueThread { issue_id, chat_id } => {
            ensure!(
                zot.workers.iter().any(|w| w.chat_id == *chat_id),
                "thread must belong to this Zapper"
            );
            let issue = find(zot, issue_id)?;
            if issue.threads.contains(chat_id) {
                return Ok(());
            }
            issue.threads.push(chat_id.clone());
            (issue_id, "thread", "Linked thread".to_owned())
        }
        ZotAction::LinkIssuePullRequest {
            issue_id,
            url,
            title,
        } => {
            // Only canonical HTTPS GitHub PR links are admitted, never arbitrary
            // executable schemes, credentials, query parameters or fragments.
            let url = canonical_pr(url)?;
            ensure!(title.len() <= 500, "pull request title exceeds 500 bytes");
            let issue = find(zot, issue_id)?;
            if issue.pull_requests.iter().any(|pr| pr.url == url) {
                return Ok(());
            }
            notify = true;
            issue.pull_requests.push(ZotIssuePullRequest {
                url: url.clone(),
                title: title.trim().to_owned(),
            });
            (
                issue_id,
                "pullRequest",
                format!("Linked pull request {url}"),
            )
        }
        ZotAction::UnlinkIssueThread { issue_id, chat_id } => {
            let issue = find(zot, issue_id)?;
            if !issue.threads.contains(chat_id) {
                return Ok(());
            }
            issue.threads.retain(|id| id != chat_id);
            (issue_id, "thread", "Unlinked thread".to_owned())
        }
        ZotAction::UnlinkIssuePullRequest { issue_id, url } => {
            let url = canonical_pr(url)?;
            let issue = find(zot, issue_id)?;
            if !issue.pull_requests.iter().any(|pr| pr.url == url) {
                return Ok(());
            }
            issue.pull_requests.retain(|pr| pr.url != url);
            (issue_id, "pullRequest", "Unlinked pull request".to_owned())
        }
        ZotAction::CommentIssue {
            issue_id,
            text,
            reply_to,
        } => {
            ensure!(
                !text.trim().is_empty() && text.len() <= 65536,
                "comment must contain 1–65536 bytes"
            );
            let issue = find(zot, issue_id)?;
            if let Some(parent) = reply_to {
                // Replies stay one level deep: answering a reply answers its thread.
                let target = issue
                    .activity
                    .iter()
                    .find(|a| a.id == *parent)
                    .context("reply target not found")?;
                reply = Some(target.reply_to.clone().unwrap_or_else(|| target.id.clone()));
            }
            notify = true;
            (issue_id, "comment", text.trim().to_owned())
        }
        ZotAction::Delegate {
            issue_id: Some(issue_id),
            worker,
            ..
        } => {
            let issue = find(zot, issue_id)?;
            ensure!(
                !issue.details.status.terminal(),
                "reopen the issue before starting work"
            );
            ensure!(
                issue.details.assigned,
                "assign the issue to this Zapper before starting work"
            );
            issue.threads.push(worker.chat_id.clone());
            issue.details.status = ZotIssueStatus::InProgress;
            (
                issue_id,
                "thread",
                "Started thread · In progress".to_owned(),
            )
        }
        _ => return Ok(()),
    };
    let issue = find(zot, id)?;
    issue.updated_at = now;
    issue.activity.push(ZotIssueActivity {
        actor: actor.map(str::to_owned),
        id: command.command_id.clone(),
        created_at: now,
        kind: kind.into(),
        text: text.clone(),
        reply_to: reply.clone(),
    });
    if notify && !coordinator {
        // A reply carries what it answers, so the Zot can act without a lookup.
        let context = reply
            .as_ref()
            .and_then(|parent| issue.activity.iter().find(|a| a.id == *parent))
            .map(|parent| {
                let quoted: String = parent.text.chars().take(280).collect();
                format!("\nIn reply to: {quoted}")
            })
            .unwrap_or_default();
        let summary = format!(
            "{}: {}\n{}{}",
            issue.code(),
            issue.details.title,
            text,
            context
        );
        tx.execute(
            "INSERT INTO zot_events(id,zot,worker,kind,summary) VALUES(?1,?2,'','issueChanged',?3)",
            params![format!("issue-{}", command.command_id), zot.id, summary],
        )?;
    }
    Ok(())
}
/// The Zot as the coordinator prompt sees it. Issues carry only their recent
/// activity and a bounded description; storage keeps everything, and the
/// coordinator reads a full issue through `zot_issue`.
pub(super) fn coordinator_view(zot: &Zot) -> Result<serde_json::Value> {
    const RECENT_ACTIVITY: usize = 6;
    const ACTIVITY_TEXT: usize = 600;
    const DESCRIPTION: usize = 4000;
    let clip = |text: &str, max: usize| -> (String, bool) {
        if text.chars().count() > max {
            (
                format!("{}…", text.chars().take(max).collect::<String>()),
                true,
            )
        } else {
            (text.to_owned(), false)
        }
    };
    let mut view = serde_json::to_value(zot)?;
    view["issues"] = zot
        .issues
        .iter()
        .map(|issue| -> Result<serde_json::Value> {
            let mut details = serde_json::to_value(&issue.details)?;
            let (description, clipped) = clip(&issue.details.description, DESCRIPTION);
            details["description"] = serde_json::json!(description);
            let skip = issue.activity.len().saturating_sub(RECENT_ACTIVITY);
            let recent: Vec<_> = issue.activity[skip..]
                .iter()
                .map(|a| {
                    let mut a = a.clone();
                    a.text = clip(&a.text, ACTIVITY_TEXT).0;
                    a
                })
                .collect();
            Ok(serde_json::json!({
                "id": issue.id,
                "code": issue.code(),
                "number": issue.number,
                "details": details,
                "descriptionClipped": clipped,
                "createdAt": issue.created_at,
                "updatedAt": issue.updated_at,
                "threads": issue.threads,
                "pullRequests": issue.pull_requests,
                "activityCount": issue.activity.len(),
                "recentActivity": recent,
            }))
        })
        .collect::<Result<Vec<_>>>()?
        .into();
    Ok(view)
}
fn find<'a>(zot: &'a mut Zot, id: &str) -> Result<&'a mut ZotIssue> {
    zot.issues
        .iter_mut()
        .find(|i| i.id == id)
        .context("issue not found")
}
fn validate(zot: &Zot, id: &str, details: &ZotIssueDetails) -> Result<()> {
    ensure!(
        !details.title.trim().is_empty() && details.title.len() <= 500,
        "issue title must contain 1–500 bytes"
    );
    ensure!(
        details.description.len() <= 1024 * 1024,
        "description exceeds 1 MiB"
    );
    ensure!(
        details.labels.len() <= 32
            && details
                .labels
                .iter()
                .all(|s| !s.trim().is_empty() && s.len() <= 80),
        "invalid issue labels"
    );
    if let Some(date) = &details.due_date {
        ensure!(
            date.len() == 10 && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok(),
            "due date must be YYYY-MM-DD"
        );
    }
    ensure!(
        details
            .relations
            .iter()
            .all(|r| r != id && zot.issues.iter().any(|i| i.id == *r)),
        "relations must reference other issues in this Zapper"
    );
    Ok(())
}
fn canonical_pr(input: &str) -> Result<String> {
    let path = input
        .trim()
        .strip_prefix("https://github.com/")
        .context("use an HTTPS GitHub pull request URL")?
        .trim_end_matches('/');
    let parts: Vec<_> = path.split('/').collect();
    ensure!(
        parts.len() == 4
            && parts[2] == "pull"
            && parts[3].parse::<u64>().is_ok_and(|n| n > 0)
            && parts[..2].iter().all(|s| !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))),
        "invalid GitHub pull request URL"
    );
    Ok(format!("https://github.com/{path}"))
}

/// Deduplicated worker reports are also persisted in every linked issue. A
/// report never claims the objective is done; failures surface as blockers.
pub(super) fn report(tx: &rusqlite::Transaction<'_>, event: &ZotEvent) -> Result<()> {
    let state: String =
        tx.query_row("SELECT state FROM zots WHERE id=?1", [&event.zot_id], |r| {
            r.get(0)
        })?;
    let mut zot: Zot = serde_json::from_str(&state)?;
    let mut changed = false;
    for issue in &mut zot.issues {
        if !issue.threads.contains(&event.worker_chat_id) {
            continue;
        }
        let now = chrono::Utc::now().timestamp_millis();
        issue.activity.push(ZotIssueActivity {
            actor: Some(event.worker_chat_id.clone()),
            id: event.id.clone(),
            created_at: now,
            kind: "workerReport".into(),
            text: format!(
                "{}\n{}",
                match event.kind.as_str() {
                    "settled" => "Worker update",
                    "failed" => "Worker failed",
                    "interrupted" => "Worker interrupted",
                    "needsInput" => "Worker needs input",
                    _ => "Worker update",
                },
                event.summary
            ),
            reply_to: None,
        });
        issue.updated_at = now;
        if !issue.details.status.terminal()
            && matches!(
                event.kind.as_str(),
                "failed" | "interrupted" | "needsInput" | "bindingPending"
            )
        {
            issue.details.status = ZotIssueStatus::Blocked;
        }
        changed = true;
    }
    if changed {
        zot.revision = zot
            .revision
            .checked_add(1)
            .context("Zot revision exhausted")?;
        tx.execute(
            "UPDATE zots SET state=?2 WHERE id=?1",
            params![zot.id, serde_json::to_string(&zot)?],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zots::tests::{command, config, create, worker};
    fn details() -> ZotIssueDetails {
        ZotIssueDetails {
            title: "Ship the fix".into(),
            description: "Pass acceptance".into(),
            assigned: true,
            ..Default::default()
        }
    }
    fn issue(store: &Zots) -> ZotIssue {
        store.get("release").unwrap().issues[0].clone()
    }
    fn add(store: &Zots) {
        store
            .command(command(
                "release",
                "issue",
                ZotAction::CreateIssue {
                    issue_id: "fix".into(),
                    details: details(),
                },
            ))
            .unwrap();
    }
    #[test]
    fn coordinator_view_keeps_recent_activity_and_flags_clipped_descriptions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let mut long = details();
        long.description = "x".repeat(5000);
        store
            .command(command(
                "release",
                "long",
                ZotAction::EditIssue {
                    issue_id: "fix".into(),
                    details: long,
                },
            ))
            .unwrap();
        for n in 0..10 {
            store
                .command(command(
                    "release",
                    &format!("comment-{n}"),
                    ZotAction::CommentIssue {
                        issue_id: "fix".into(),
                        text: format!("note {n} {}", "y".repeat(900)),
                        reply_to: None,
                    },
                ))
                .unwrap();
        }
        let first = issue(&store).activity[2].id.clone();
        store
            .command(command(
                "release",
                "reply",
                ZotAction::CommentIssue {
                    issue_id: "fix".into(),
                    text: "Answering".into(),
                    reply_to: Some(first.clone()),
                },
            ))
            .unwrap();
        // Replies attach to their thread's root, never to another reply.
        let stored = issue(&store);
        let reply = stored.activity.last().unwrap().id.clone();
        assert_eq!(
            stored.activity.last().unwrap().reply_to,
            Some(first.clone())
        );
        store
            .command(command(
                "release",
                "reply-to-reply",
                ZotAction::CommentIssue {
                    issue_id: "fix".into(),
                    text: "Again".into(),
                    reply_to: Some(reply),
                },
            ))
            .unwrap();
        let stored = issue(&store);
        assert_eq!(stored.activity.last().unwrap().reply_to, Some(first));
        // Storage keeps everything; the prompt view is bounded.
        assert_eq!(stored.activity.len(), 14);
        assert_eq!(stored.details.description.len(), 5000);
        let view = coordinator_view(&store.get("release").unwrap()).unwrap();
        let fix = &view["issues"][0];
        assert_eq!(fix["descriptionClipped"], true);
        assert_eq!(fix["activityCount"], 14);
        assert_eq!(fix["recentActivity"].as_array().unwrap().len(), 6);
        assert!(
            fix["recentActivity"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["text"].as_str().unwrap().chars().count() <= 601)
        );
        assert!(
            fix["details"]["description"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 4001
        );
    }
    #[test]
    fn unlinking_is_reversible_without_releasing_workers_or_duplicate_activity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        store
            .command(command(
                "release",
                "adopt",
                ZotAction::AdoptWorker { worker: worker() },
            ))
            .unwrap();
        store
            .command(command(
                "release",
                "link",
                ZotAction::LinkIssueThread {
                    issue_id: "fix".into(),
                    chat_id: "worker".into(),
                },
            ))
            .unwrap();
        store
            .command(command(
                "release",
                "pr",
                ZotAction::LinkIssuePullRequest {
                    issue_id: "fix".into(),
                    url: "https://github.com/team/repo/pull/123".into(),
                    title: "Fix".into(),
                },
            ))
            .unwrap();
        let unlink = command(
            "release",
            "unlink",
            ZotAction::UnlinkIssueThread {
                issue_id: "fix".into(),
                chat_id: "worker".into(),
            },
        );
        store.command(unlink.clone()).unwrap();
        store.command(unlink).unwrap();
        store
            .command(command(
                "release",
                "unlink-pr",
                ZotAction::UnlinkIssuePullRequest {
                    issue_id: "fix".into(),
                    url: "https://github.com/team/repo/pull/123".into(),
                },
            ))
            .unwrap();
        assert!(issue(&store).threads.is_empty());
        assert!(issue(&store).pull_requests.is_empty());
        assert_eq!(store.get("release").unwrap().workers.len(), 1);
        let activity = issue(&store).activity.len();
        store
            .command(command(
                "release",
                "unlink-again",
                ZotAction::UnlinkIssueThread {
                    issue_id: "fix".into(),
                    chat_id: "worker".into(),
                },
            ))
            .unwrap();
        assert_eq!(issue(&store).activity.len(), activity);
    }
    #[test]
    fn coordinator_mutations_do_not_create_self_notification_loops() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        store
            .command_as(
                command(
                    "release",
                    "self-issue",
                    ZotAction::CreateIssue {
                        issue_id: "fix".into(),
                        details: details(),
                    },
                ),
                Some("Release"),
                true,
            )
            .unwrap();
        assert!(store.events("release").unwrap().is_empty());
        let comment = command(
            "release",
            "worker-report",
            ZotAction::CommentIssue {
                issue_id: "fix".into(),
                text: "Ready for review".into(),
                reply_to: None,
            },
        );
        store
            .command_as(comment.clone(), Some("worker"), false)
            .unwrap();
        store.command_as(comment, Some("worker"), false).unwrap();
        assert_eq!(store.events("release").unwrap().len(), 1);
        assert_eq!(issue(&store).activity.len(), 2);
    }
    #[test]
    fn issues_persist_with_receipts_and_reject_stale_edits() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let rev = store.get("release").unwrap().revision;
        let mut changed = details();
        changed.status = ZotIssueStatus::InReview;
        let mut edit = command(
            "release",
            "edit",
            ZotAction::EditIssue {
                issue_id: "fix".into(),
                details: changed.clone(),
            },
        );
        edit.expected_revision = Some(rev);
        let result = store.command(edit.clone()).unwrap();
        let mut stale = edit.clone();
        stale.command_id = "stale".into();
        assert!(store.command(stale).is_err());
        assert_eq!(issue(&store).details.status, ZotIssueStatus::InReview);
        drop(store);
        let store = Zots::open(dir.path(), "local").unwrap();
        assert_eq!(store.command(edit).unwrap(), result);
        assert_eq!(issue(&store).number, 1);
        assert_eq!(issue(&store).activity.len(), 2);
        assert_eq!(store.events("release").unwrap().len(), 2);
    }
    #[test]
    fn delegation_links_atomically_and_worker_reports_never_finish_issues() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let delegate = command(
            "release",
            "delegate",
            ZotAction::Delegate {
                issue_id: Some("fix".into()),
                worker: worker(),
                model: config().coordinator_model,
                base: "main".into(),
            },
        );
        store.command(delegate.clone()).unwrap();
        store.command(delegate).unwrap();
        assert_eq!(issue(&store).threads, vec!["worker"]);
        assert_eq!(issue(&store).details.status, ZotIssueStatus::InProgress);
        assert_eq!(store.outbox().unwrap().len(), 1);
        let mut event = ZotEvent {
            id: "report".into(),
            zot_id: "release".into(),
            worker_chat_id: "worker".into(),
            kind: "settled".into(),
            summary: "Tests passed".into(),
            acknowledged: false,
        };
        store.record_event(&event).unwrap();
        store.record_event(&event).unwrap();
        assert_eq!(issue(&store).activity.len(), 3);
        assert_eq!(issue(&store).details.status, ZotIssueStatus::InProgress);
        event.id = "failure".into();
        event.kind = "failed".into();
        store.record_event(&event).unwrap();
        assert_eq!(issue(&store).details.status, ZotIssueStatus::Blocked);
    }
    #[test]
    fn invalid_links_and_delegations_do_not_partially_mutate_issues() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let before = store.get("release").unwrap();
        assert!(
            store
                .command(command(
                    "release",
                    "bad-thread",
                    ZotAction::LinkIssueThread {
                        issue_id: "fix".into(),
                        chat_id: "foreign".into()
                    }
                ))
                .is_err()
        );
        for url in [
            "javascript:alert(1)",
            "https://github.com/a/b/pull/0",
            "https://github.com/../b/pull/2",
            "https://github.com/a/b/pull/2?token=x",
        ] {
            assert!(
                store
                    .command(command(
                        "release",
                        "bad-pr",
                        ZotAction::LinkIssuePullRequest {
                            issue_id: "fix".into(),
                            url: url.into(),
                            title: "PR".into()
                        }
                    ))
                    .is_err()
            );
        }
        let mut outside = worker();
        outside.space_id = "outside".into();
        assert!(
            store
                .command(command(
                    "release",
                    "bad-delegate",
                    ZotAction::Delegate {
                        issue_id: Some("fix".into()),
                        worker: outside,
                        model: config().coordinator_model,
                        base: "main".into()
                    }
                ))
                .is_err()
        );
        assert_eq!(store.get("release").unwrap(), before);
        assert!(store.outbox().unwrap().is_empty());
        store
            .command(command(
                "release",
                "pr",
                ZotAction::LinkIssuePullRequest {
                    issue_id: "fix".into(),
                    url: "https://github.com/team/repo/pull/123/".into(),
                    title: "The fix".into(),
                },
            ))
            .unwrap();
        assert_eq!(
            issue(&store).pull_requests[0].url,
            "https://github.com/team/repo/pull/123"
        );
    }
    #[test]
    fn old_zots_default_to_no_issues_and_relations_stay_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut json = serde_json::to_value(store.get("release").unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("issues");
        assert!(
            serde_json::from_value::<Zot>(json)
                .unwrap()
                .issues
                .is_empty()
        );
        let mut invalid = details();
        invalid.relations.push("foreign".into());
        assert!(
            store
                .command(command(
                    "release",
                    "relation",
                    ZotAction::CreateIssue {
                        issue_id: "fix".into(),
                        details: invalid
                    }
                ))
                .is_err()
        );
        add(&store);
        let mut invalid = details();
        invalid.due_date = Some("2026-02-30".into());
        assert!(
            store
                .command(command(
                    "release",
                    "date",
                    ZotAction::EditIssue {
                        issue_id: "fix".into(),
                        details: invalid
                    }
                ))
                .is_err()
        );
    }
}

#[cfg(test)]
mod attribution_tests {
    use super::*;
    use crate::zots::tests::{command, create, worker};
    #[test]
    fn activity_notices_keep_host_instructions_out_of_the_visible_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let zot = store.get("release").unwrap();
        let mut request: zuse_protocol::RunRequest = serde_json::from_value(serde_json::json!({
            "prompt": zuse_protocol::presentation::zot_message::activity_from(&zot, "Implementation thread", "Activity update\nVerification passed; ready for review."),
            "cwd": "/tmp", "sandbox": "workspace-write"
        })).unwrap();
        store
            .prepare_request(&zot.coordinator_chat_id, &mut request)
            .unwrap();
        let (sender, visible) =
            zuse_protocol::presentation::zot_message::parse(&request.prompt).unwrap();
        assert_eq!(sender.name, "Implementation thread");
        assert!(sender.activity);
        assert_eq!(
            visible,
            "Activity update\nVerification passed; ready for review."
        );
        assert!(request.prompt.contains("acknowledge handled events"));
        assert!(request.prompt.contains("untrusted evidence"));
    }
    #[test]
    fn worker_context_keeps_attribution_and_hides_engine_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        store
            .command(command(
                "release",
                "adopt",
                ZotAction::AdoptWorker { worker: worker() },
            ))
            .unwrap();
        let zot = store.get("release").unwrap();
        let mut request: zuse_protocol::RunRequest=serde_json::from_value(serde_json::json!({"prompt":zuse_protocol::presentation::zot_message::envelope(&zot,"Implement the issue"),"cwd":"/tmp","sandbox":"workspace-write"})).unwrap();
        store.prepare_request("worker", &mut request).unwrap();
        let (sender, display) =
            zuse_protocol::presentation::zot_message::parse(&request.prompt).unwrap();
        assert_eq!(sender.name, "Release");
        assert_eq!(display, "Implement the issue");
        assert!(request.prompt.contains("You are a worker for Zot release"));
        let once = request.prompt.clone();
        store.prepare_request("worker", &mut request).unwrap();
        assert_eq!(request.prompt, once);
    }
}
