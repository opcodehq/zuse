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
    let zot_workers: Vec<String> = zot.workers.iter().map(|w| w.chat_id.clone()).collect();
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
            let mut details = details.clone();
            if coordinator {
                ensure!(
                    details.status == ZotIssueStatus::Backlog
                        || !details.done_criteria.trim().is_empty(),
                    "add doneCriteria: what must be true for this issue to be done"
                );
                ensure!(
                    !matches!(details.current_owner(), Some(ZotIssueOwner::User)),
                    "create the issue for yourself, then hand it over with askUser"
                );
                ensure!(
                    matches!(
                        details.status,
                        ZotIssueStatus::Backlog | ZotIssueStatus::Todo
                    ),
                    "new issues start in backlog or todo; delegate moves one to inProgress"
                );
            }
            details.normalize(false);
            let number = zot
                .issues
                .iter()
                .map(|i| i.number)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .context("issue numbers exhausted")?;
            notify = details.assigned && !details.status.terminal();
            zot.issues.push(ZotIssue {
                id: issue_id.clone(),
                number,
                details,
                created_at: now,
                updated_at: now,
                threads: vec![],
                pull_requests: vec![],
                activity: vec![],
            });
            (issue_id, "created", "Created the issue".to_owned())
        }
        ZotAction::EditIssue { issue_id, details } => {
            validate(zot, issue_id, details)?;
            let child_in_progress = zot.issues.iter().any(|i| {
                i.details.parent.as_deref() == Some(issue_id.as_str())
                    && i.details.status == ZotIssueStatus::InProgress
            });
            let issue = find(zot, issue_id)?;
            let mut details = details.clone();
            if coordinator {
                transition(issue, &details, child_in_progress)?;
            }
            details.normalize(!issue.threads.is_empty());
            if issue.details == details {
                return Ok(());
            }
            notify = details.assigned && !details.status.terminal();
            let mut changes = vec![];
            let (before, after) = (issue.details.current_owner(), details.current_owner());
            if before != after {
                changes.push(match &after {
                    Some(owner) => format!("Owner: {}", owner_label(owner)),
                    None => "Removed owner".to_owned(),
                });
                if before == Some(ZotIssueOwner::User) {
                    changes.push("Decision received".to_owned());
                }
            }
            if issue.details.status != details.status {
                changes.push(format!("Status changed to {}", details.status.label()));
            }
            if issue.details.priority != details.priority {
                changes.push(format!("Priority changed to {}", details.priority.label()));
            }
            if changes.is_empty() {
                changes.push("Updated issue details".into());
            }
            issue.details = details;
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
            // A person answering an issue that waits on them hands it back:
            // to the thread doing the work, else to the Zapper.
            if !coordinator && issue.details.status == ZotIssueStatus::NeedsYou {
                hand_back(issue, &zot_workers);
            }
            (issue_id, "comment", text.trim().to_owned())
        }
        ZotAction::AskUser {
            issue_id,
            question,
            recommendation,
        } => {
            ensure!(
                !question.trim().is_empty() && question.len() <= 8192,
                "question must contain 1–8192 bytes"
            );
            let issue = find(zot, issue_id)?;
            ensure!(
                !issue.details.status.terminal(),
                "reopen the issue before asking about it"
            );
            issue.details.owner = Some(ZotIssueOwner::User);
            issue.details.normalize(!issue.threads.is_empty());
            notify = !coordinator;
            let text = match recommendation.as_deref().map(str::trim) {
                Some(r) if !r.is_empty() => {
                    format!("{}\n\nRecommendation: {r}", question.trim())
                }
                _ => question.trim().to_owned(),
            };
            (issue_id, "question", text)
        }
        ZotAction::CloseIssue {
            issue_id,
            outcome,
            note,
        } => {
            ensure!(
                !note.trim().is_empty() && note.len() <= 8192,
                "say what happened in the note (1–8192 bytes)"
            );
            let issue = find(zot, issue_id)?;
            let (status, verb) = match outcome {
                ZotIssueOutcome::Merged => {
                    ensure!(
                        !issue.pull_requests.is_empty(),
                        "link the merged pull request before closing the issue as merged"
                    );
                    (ZotIssueStatus::Done, "Merged")
                }
                ZotIssueOutcome::Dropped => (ZotIssueStatus::Canceled, "Dropped"),
            };
            issue.details.status = status;
            notify = !coordinator;
            (issue_id, "closed", format!("{verb} · {}", note.trim()))
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
            issue.details.owner = Some(ZotIssueOwner::Thread {
                chat_id: worker.chat_id.clone(),
            });
            issue.details.status = ZotIssueStatus::InProgress;
            issue.details.normalize(true);
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
    // The purpose reaches the prompt as INSTRUCTIONS.md; once is enough.
    if let Some(config) = view["config"].as_object_mut() {
        config.remove("purpose");
    }
    view["issues"] = zot
        .issues
        .iter()
        .map(|issue| -> Result<serde_json::Value> {
            // Closed work is history: one line each keeps every turn's
            // prompt about the open board, not the archive.
            if issue.details.status.terminal() {
                return Ok(serde_json::json!({
                    "id": issue.id,
                    "code": issue.code(),
                    "title": issue.details.title,
                    "status": issue.details.status,
                }));
            }
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
            let mut view = serde_json::json!({
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
            });
            // Its thread waits for a slot: the stored inProgress isn't running yet.
            if let Some(worker) = zot.issue_queued_worker(issue) {
                view["queued"] = serde_json::json!(true);
                view["queuePosition"] = serde_json::json!(worker.queue_position);
            }
            Ok(view)
        })
        .collect::<Result<Vec<_>>>()?
        .into();
    Ok(view)
}
/// Status moves the coordinator may make itself. Each one needs the evidence
/// that makes it true, so the board can't drift ahead of the work.
/// A parent issue is in progress while one of its children is.
fn transition(issue: &ZotIssue, next: &ZotIssueDetails, child_in_progress: bool) -> Result<()> {
    let before = &issue.details;
    if next.status != before.status {
        ensure!(
            !next.status.terminal(),
            "close issues with closeIssue (outcome merged or dropped, plus a note)"
        );
        match next.status {
            ZotIssueStatus::InReview => ensure!(
                !issue.pull_requests.is_empty(),
                "link the pull request before moving the issue to inReview"
            ),
            ZotIssueStatus::InProgress => ensure!(
                !issue.threads.is_empty() || child_in_progress,
                "an issue is inProgress only while a thread works on it: delegate {{issueId: \"{}\"}} (which moves it to inProgress for you), or, for a parent issue, start a thread on one of its child issues (details.parent: \"{}\") first",
                issue.id,
                issue.id
            ),
            ZotIssueStatus::NeedsYou => {
                bail!("hand the issue to the user with askUser")
            }
            _ => {}
        }
    }
    ensure!(
        next.current_owner() == before.current_owner()
            || next.current_owner() != Some(ZotIssueOwner::User),
        "hand the issue to the user with askUser"
    );
    ensure!(
        before.done_criteria.trim().is_empty() || !next.done_criteria.trim().is_empty(),
        "keep the issue's doneCriteria; edit it rather than clearing it"
    );
    Ok(())
}
/// The user answered: the newest linked thread still working for the Zapper
/// takes the issue back, else the Zapper does.
fn hand_back(issue: &mut ZotIssue, workers: &[String]) {
    let thread = issue
        .threads
        .iter()
        .rev()
        .find(|id| workers.contains(id))
        .cloned();
    issue.details.owner = Some(match thread {
        Some(chat_id) => ZotIssueOwner::Thread { chat_id },
        None => ZotIssueOwner::Zapper,
    });
    issue.details.normalize(!issue.threads.is_empty());
}
fn owner_label(owner: &ZotIssueOwner) -> &'static str {
    match owner {
        ZotIssueOwner::Zapper => "Zapper",
        ZotIssueOwner::User => "You",
        ZotIssueOwner::Thread { .. } => "Thread",
    }
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
        !details.relations.iter().any(|r| r == id),
        "relations must reference other issues, not the issue itself"
    );
    let unknown: Vec<&str> = details
        .relations
        .iter()
        .filter(|r| !zot.issues.iter().any(|i| i.id == **r))
        .map(String::as_str)
        .collect();
    ensure!(
        unknown.is_empty(),
        "relations must reference existing issues in this Zapper; unknown: {}. Create those issues first (createIssue, one command each), then add the relation with editIssue, or leave relations empty for now",
        unknown.join(", ")
    );
    if let Some(ZotIssueOwner::Thread { chat_id }) = &details.owner {
        ensure!(
            zot.workers.iter().any(|w| w.chat_id == *chat_id),
            "an owning thread must belong to this Zapper"
        );
    }
    if let Some(parent) = &details.parent {
        ensure!(parent != id, "an issue can't be its own parent");
        ensure!(
            zot.issues.iter().any(|i| i.id == *parent),
            "parent issue {parent} doesn't exist yet: create it first (createIssue), then create or edit this issue with details.parent"
        );
    }
    ensure!(
        details.done_criteria.len() <= 65536 && details.out_of_scope.len() <= 65536,
        "doneCriteria and outOfScope are limited to 64 KiB each"
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

/// The status line a thread's final report starts with.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReportStatus {
    Done,
    Blocked,
}
/// Reads `Status: done` / `Status: blocked` (or `needs-decision`) from the
/// first lines of a report. Free-form reports carry no status.
pub(super) fn report_status(summary: &str) -> Option<ReportStatus> {
    summary.lines().take(5).find_map(|line| {
        let value = line
            .trim()
            .trim_start_matches(['*', '#', ' '])
            .strip_prefix("Status:")
            .or_else(|| line.trim().strip_prefix("status:"))?
            .trim()
            .trim_matches('*')
            .to_ascii_lowercase();
        if value.starts_with("done") {
            Some(ReportStatus::Done)
        } else if value.starts_with("blocked") || value.starts_with("needs") {
            Some(ReportStatus::Blocked)
        } else {
            None
        }
    })
}
/// Deduplicated worker reports are also persisted in every linked issue. A
/// report never claims the objective is done; failures surface as blockers.
pub(super) fn report(tx: &rusqlite::Transaction<'_>, event: &ZotEvent) -> Result<()> {
    let state: String =
        tx.query_row("SELECT state FROM zots WHERE id=?1", [&event.zot_id], |r| {
            r.get(0)
        })?;
    let mut zot: Zot = serde_json::from_str(&state)?;
    let before = zot.clone();
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
        let reported = report_status(&event.summary);
        if !issue.details.status.terminal()
            && issue.details.status != ZotIssueStatus::NeedsYou
            && (matches!(
                event.kind.as_str(),
                "failed" | "interrupted" | "needsInput" | "bindingPending"
            ) || matches!(reported, Some(ReportStatus::Blocked)))
        {
            // A blocked thread hands the issue back to the Zapper to unblock.
            issue.details.status = ZotIssueStatus::Blocked;
            issue.details.owner = Some(ZotIssueOwner::Zapper);
            issue.details.normalize(true);
        }
        changed = true;
    }
    if changed {
        zot.revision = zot
            .revision
            .checked_add(1)
            .context("Zapper revision exhausted")?;
        record_changes(tx, &before, &zot)?;
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
    fn as_zapper(store: &Zots, id: &str, action: ZotAction) -> Result<Option<Zot>> {
        store.command_as(command("release", id, action), Some("Release"), true)
    }
    #[test]
    fn coordinator_status_moves_need_their_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut todo = details();
        todo.status = ZotIssueStatus::Todo;
        let create_fix = |id: &str, details: ZotIssueDetails| {
            as_zapper(
                &store,
                id,
                ZotAction::CreateIssue {
                    issue_id: "fix".into(),
                    details,
                },
            )
        };
        let err = create_fix("no-criteria", todo.clone()).unwrap_err();
        assert!(err.to_string().contains("doneCriteria"), "{err}");
        todo.done_criteria = "The login form accepts SSO".into();
        create_fix("with-criteria", todo.clone()).unwrap();
        assert_eq!(
            issue(&store).details.current_owner(),
            Some(ZotIssueOwner::Zapper)
        );
        let edit = |id: &str, status: ZotIssueStatus| {
            let mut next = issue(&store).details;
            next.status = status;
            as_zapper(
                &store,
                id,
                ZotAction::EditIssue {
                    issue_id: "fix".into(),
                    details: next,
                },
            )
        };
        for (status, expected) in [
            (ZotIssueStatus::InReview, "link the pull request"),
            (ZotIssueStatus::InProgress, "start a thread"),
            (ZotIssueStatus::Done, "closeIssue"),
            (ZotIssueStatus::NeedsYou, "askUser"),
        ] {
            let err = edit(&format!("to-{status:?}"), status).unwrap_err();
            assert!(err.to_string().contains(expected), "{status:?}: {err}");
        }
        let err = as_zapper(
            &store,
            "merge-without-pr",
            ZotAction::CloseIssue {
                issue_id: "fix".into(),
                outcome: ZotIssueOutcome::Merged,
                note: "Shipped".into(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("link the merged pull request"));
        as_zapper(
            &store,
            "drop",
            ZotAction::CloseIssue {
                issue_id: "fix".into(),
                outcome: ZotIssueOutcome::Dropped,
                note: "Superseded by the new login flow".into(),
            },
        )
        .unwrap();
        let closed = issue(&store);
        assert_eq!(closed.details.status, ZotIssueStatus::Canceled);
        assert!(
            closed
                .activity
                .last()
                .unwrap()
                .text
                .starts_with("Dropped · Superseded")
        );
    }
    #[test]
    fn a_parent_shows_in_progress_while_a_child_is() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut parent = details();
        parent.status = ZotIssueStatus::Todo;
        parent.done_criteria = "Both parts ship".into();
        as_zapper(
            &store,
            "parent",
            ZotAction::CreateIssue {
                issue_id: "parent".into(),
                details: parent.clone(),
            },
        )
        .unwrap();
        let mut child = parent.clone();
        child.parent = Some("parent".into());
        as_zapper(
            &store,
            "child",
            ZotAction::CreateIssue {
                issue_id: "child".into(),
                details: child,
            },
        )
        .unwrap();
        let mut moving = parent.clone();
        moving.status = ZotIssueStatus::InProgress;
        let move_parent = |id: &str| {
            as_zapper(
                &store,
                id,
                ZotAction::EditIssue {
                    issue_id: "parent".into(),
                    details: moving.clone(),
                },
            )
        };
        let err = move_parent("too-early").unwrap_err().to_string();
        assert!(err.contains("delegate {issueId: \"parent\"}"), "{err}");
        assert!(err.contains("child issues"), "{err}");
        as_zapper(
            &store,
            "zot-worker-start",
            ZotAction::Delegate {
                issue_id: Some("child".into()),
                worker: ZotWorker {
                    chat_id: "zot-worker-start".into(),
                    ..worker()
                },
                model: config().coordinator_model,
                base: "origin/main".into(),
            },
        )
        .unwrap();
        move_parent("with-child").unwrap();
        let zot = store.get("release").unwrap();
        let parent = zot.issues.iter().find(|i| i.id == "parent").unwrap();
        assert_eq!(parent.details.status, ZotIssueStatus::InProgress);
    }
    #[test]
    fn unknown_relations_and_parents_say_to_create_them_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zots::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut related = details();
        related.relations = vec!["later".into()];
        let err = store
            .command(command(
                "release",
                "related",
                ZotAction::CreateIssue {
                    issue_id: "fix".into(),
                    details: related,
                },
            ))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown: later"), "{err}");
        assert!(err.contains("Create those issues first"), "{err}");
        let mut child = details();
        child.parent = Some("later".into());
        let err = store
            .command(command(
                "release",
                "child",
                ZotAction::CreateIssue {
                    issue_id: "fix".into(),
                    details: child,
                },
            ))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("parent issue later doesn't exist yet"),
            "{err}"
        );
    }
    #[test]
    fn asking_the_user_hands_over_the_issue_and_their_answer_hands_it_back() {
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
        let before = store.events("release").unwrap().len();
        as_zapper(
            &store,
            "ask",
            ZotAction::AskUser {
                issue_id: "fix".into(),
                question: "Return 401 or 403 for a failed login?".into(),
                recommendation: Some("401, matching the API".into()),
            },
        )
        .unwrap();
        let asked = issue(&store);
        assert_eq!(asked.details.current_owner(), Some(ZotIssueOwner::User));
        assert_eq!(asked.details.status, ZotIssueStatus::NeedsYou);
        assert!(!asked.details.assigned);
        assert_eq!(asked.activity.last().unwrap().kind, "question");
        assert_eq!(store.events("release").unwrap().len(), before);
        store
            .command(command(
                "release",
                "answer",
                ZotAction::CommentIssue {
                    issue_id: "fix".into(),
                    text: "401".into(),
                    reply_to: None,
                },
            ))
            .unwrap();
        let answered = issue(&store);
        assert_eq!(
            answered.details.current_owner(),
            Some(ZotIssueOwner::Thread {
                chat_id: "worker".into()
            })
        );
        assert_eq!(answered.details.status, ZotIssueStatus::InProgress);
        assert!(answered.details.assigned);
        assert_eq!(store.events("release").unwrap().len(), before + 1);
    }
    #[test]
    fn reports_carry_a_typed_status_line() {
        assert_eq!(
            report_status("Status: done\nOpened #12"),
            Some(ReportStatus::Done)
        );
        assert_eq!(
            report_status("**Status: blocked**\nNeed the staging key"),
            Some(ReportStatus::Blocked)
        );
        assert_eq!(
            report_status("Status: needs-decision"),
            Some(ReportStatus::Blocked)
        );
        assert_eq!(report_status("I looked around the code."), None);
    }
    #[test]
    fn issues_saved_before_owners_read_assigned_as_the_zapper() {
        let legacy: ZotIssueDetails = serde_json::from_str(
            r#"{"title":"Old","description":"","assigned":true,"dueDate":null}"#,
        )
        .unwrap();
        assert_eq!(legacy.current_owner(), Some(ZotIssueOwner::Zapper));
        assert!(legacy.done_criteria.is_empty());
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
            .prepare_request(
                &zot.coordinator_chat_id,
                zuse_protocol::HarnessId::ClaudeCode,
                &mut request,
            )
            .unwrap();
        let (sender, visible) =
            zuse_protocol::presentation::zot_message::parse(&request.prompt).unwrap();
        assert_eq!(sender.name, "Implementation thread");
        assert!(sender.activity);
        assert_eq!(
            visible,
            "Activity update\nVerification passed; ready for review."
        );
        // The standing instructions ride the system prompt, outside the turn.
        let brief = request.model_options[zuse_providers::claude::SYSTEM_PROMPT_OPTION]
            .as_str()
            .unwrap();
        assert!(brief.contains("zapper_acknowledge"));
        assert!(brief.contains("untrusted evidence"));
        assert!(request.prompt.contains("## Board (revision"));
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
        store
            .prepare_request("worker", zuse_protocol::HarnessId::ClaudeCode, &mut request)
            .unwrap();
        let (sender, display) =
            zuse_protocol::presentation::zot_message::parse(&request.prompt).unwrap();
        assert_eq!(sender.name, "Release");
        assert_eq!(display, "Implement the issue");
        assert!(
            request
                .prompt
                .contains("You are a thread working for the Zapper \"Release\" (zotId release")
        );
        assert!(!request.prompt.contains("Zot "));
        let once = request.prompt.clone();
        store
            .prepare_request("worker", zuse_protocol::HarnessId::ClaudeCode, &mut request)
            .unwrap();
        assert_eq!(request.prompt, once);
    }
}
