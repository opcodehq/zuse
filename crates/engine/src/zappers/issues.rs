//! Issue changes share the Zapper receipt transaction and revision check.
use super::*;

pub(super) fn apply(
    zapper: &mut Zapper,
    command: &ZapperCommand,
    tx: &rusqlite::Transaction<'_>,
    actor: Option<&str>,
    coordinator: bool,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let zapper_workers: Vec<String> = zapper.workers.iter().map(|w| w.chat_id.clone()).collect();
    let mut notify = false;
    let mut reply: Option<String> = None;
    let mut changes: Vec<ZapperIssueChange> = vec![];
    let mut delivery: Option<ZapperIssueDelivery> = None;
    let mut asked_for: Option<String> = None;
    let (status, coordinator_chat) = (zapper.status, zapper.coordinator_chat_id.clone());
    let (id, kind, text) = match &command.action {
        ZapperAction::CreateIssue { issue_id, details } => {
            valid_id(issue_id)?;
            ensure!(
                !zapper.issues.iter().any(|i| i.id == *issue_id),
                "issue already exists"
            );
            validate(zapper, issue_id, details)?;
            let mut details = details.clone();
            if coordinator {
                ensure!(
                    details.status == ZapperIssueStatus::Backlog
                        || !details.done_criteria.trim().is_empty(),
                    "add doneCriteria: what must be true for this issue to be done"
                );
                ensure!(
                    !matches!(details.current_owner(), Some(ZapperIssueOwner::User)),
                    "create the issue for yourself, then hand it over with askUser"
                );
                ensure!(
                    matches!(
                        details.status,
                        ZapperIssueStatus::Backlog | ZapperIssueStatus::Todo
                    ),
                    "new issues start in backlog or todo; delegate moves one to inProgress"
                );
            }
            details.normalize(false);
            let number = zapper
                .issues
                .iter()
                .map(|i| i.number)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .context("issue numbers exhausted")?;
            notify = details.assigned && !details.status.terminal();
            zapper.issues.push(ZapperIssue {
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
        ZapperAction::EditIssue { issue_id, details } => {
            validate(zapper, issue_id, details)?;
            let child_in_progress = zapper.issues.iter().any(|i| {
                i.details.parent.as_deref() == Some(issue_id.as_str())
                    && i.details.status == ZapperIssueStatus::InProgress
            });
            let issue = find(zapper, issue_id)?;
            let mut details = details.clone();
            if coordinator {
                transition(issue, &details, child_in_progress)?;
            }
            details.normalize(!issue.threads.is_empty());
            if issue.details == details {
                return Ok(());
            }
            notify = details.assigned && !details.status.terminal();
            let mut summary = vec![];
            let (before, after) = (issue.details.current_owner(), details.current_owner());
            if before != after {
                summary.push(match &after {
                    Some(owner) => format!("Owner: {}", owner_label(owner)),
                    None => "Removed owner".to_owned(),
                });
                if before == Some(ZapperIssueOwner::User) {
                    summary.push("Decision received".to_owned());
                }
                changes.push(change(
                    "owner",
                    before.as_ref().map(owner_value),
                    after.as_ref().map(owner_value),
                ));
            }
            if issue.details.status != details.status {
                summary.push(format!("Status changed to {}", details.status.label()));
                changes.push(change(
                    "status",
                    Some(wire(&issue.details.status)),
                    Some(wire(&details.status)),
                ));
            }
            if issue.details.priority != details.priority {
                summary.push(format!("Priority changed to {}", details.priority.label()));
                changes.push(change(
                    "priority",
                    Some(wire(&issue.details.priority)),
                    Some(wire(&details.priority)),
                ));
            }
            let old = &issue.details;
            if old.title != details.title {
                changes.push(change(
                    "title",
                    Some(old.title.clone()),
                    Some(details.title.clone()),
                ));
            }
            if old.due_date != details.due_date {
                changes.push(change(
                    "dueDate",
                    old.due_date.clone(),
                    details.due_date.clone(),
                ));
            }
            if old.labels != details.labels {
                let joined = |labels: &[String]| (!labels.is_empty()).then(|| labels.join(", "));
                changes.push(change(
                    "labels",
                    joined(&old.labels),
                    joined(&details.labels),
                ));
            }
            for (field, before, after) in [
                ("description", &old.description, &details.description),
                ("doneCriteria", &old.done_criteria, &details.done_criteria),
                ("outOfScope", &old.out_of_scope, &details.out_of_scope),
            ] {
                if before != after {
                    changes.push(change(field, None, None));
                }
            }
            if summary.is_empty() {
                summary.push("Updated issue details".into());
                if changes.is_empty() {
                    changes.push(change("details", None, None));
                }
            }
            issue.details = details;
            (issue_id, "updated", summary.join(" · "))
        }
        ZapperAction::LinkIssueThread { issue_id, chat_id } => {
            ensure!(
                zapper.workers.iter().any(|w| w.chat_id == *chat_id),
                "thread must belong to this Zapper"
            );
            let issue = find(zapper, issue_id)?;
            if issue.threads.contains(chat_id) {
                return Ok(());
            }
            issue.threads.push(chat_id.clone());
            changes.push(change("thread", None, Some(chat_id.clone())));
            (issue_id, "thread", format!("Linked thread {chat_id}"))
        }
        ZapperAction::LinkIssuePullRequest {
            issue_id,
            url,
            title,
        } => {
            // Only canonical HTTPS GitHub PR links are admitted, never arbitrary
            // executable schemes, credentials, query parameters or fragments.
            let url = canonical_pr(url)?;
            ensure!(title.len() <= 500, "pull request title exceeds 500 bytes");
            let issue = find(zapper, issue_id)?;
            if issue.pull_requests.iter().any(|pr| pr.url == url) {
                return Ok(());
            }
            notify = true;
            changes.push(change("pullRequest", None, Some(url.clone())));
            issue.pull_requests.push(ZapperIssuePullRequest {
                url: url.clone(),
                title: title.trim().to_owned(),
            });
            (
                issue_id,
                "pullRequest",
                format!("Linked pull request {url}"),
            )
        }
        ZapperAction::UnlinkIssueThread { issue_id, chat_id } => {
            let issue = find(zapper, issue_id)?;
            if !issue.threads.contains(chat_id) {
                return Ok(());
            }
            issue.threads.retain(|id| id != chat_id);
            changes.push(change("thread", Some(chat_id.clone()), None));
            (issue_id, "thread", format!("Unlinked thread {chat_id}"))
        }
        ZapperAction::UnlinkIssuePullRequest { issue_id, url } => {
            let url = canonical_pr(url)?;
            let issue = find(zapper, issue_id)?;
            if !issue.pull_requests.iter().any(|pr| pr.url == url) {
                return Ok(());
            }
            issue.pull_requests.retain(|pr| pr.url != url);
            changes.push(change("pullRequest", Some(url.clone()), None));
            (
                issue_id,
                "pullRequest",
                format!("Unlinked pull request {url}"),
            )
        }
        ZapperAction::CommentIssue {
            issue_id,
            text,
            reply_to,
        } => {
            ensure!(
                !text.trim().is_empty() && text.len() <= 65536,
                "comment must contain 1–65536 bytes"
            );
            // Only the owner's authenticated scope arrives without an actor;
            // coordinator and worker credentials always carry theirs.
            let person = actor.is_none() && !coordinator;
            let answered_since = issue_revision(tx, &zapper.id, issue_id)?;
            let issue = find(zapper, issue_id)?;
            let target = match reply_to {
                Some(parent) => Some(
                    issue
                        .activity
                        .iter()
                        .find(|a| a.id == *parent)
                        .cloned()
                        .context("reply target not found")?,
                ),
                None => None,
            };
            // Replies stay one level deep: answering a reply answers its thread.
            reply = target
                .as_ref()
                .map(|t| t.reply_to.clone().unwrap_or_else(|| t.id.clone()));
            notify = true;
            let waiting = issue.details.status == ZapperIssueStatus::NeedsYou;
            if person {
                let recipient = recipient(
                    issue,
                    target.as_ref(),
                    waiting,
                    &zapper_workers,
                    command.expected_revision,
                    answered_since,
                    &coordinator_chat,
                );
                delivery = match recipient {
                    Recipient::Thread(chat) => Some(route(
                        tx,
                        &command.zapper_id,
                        status,
                        issue,
                        &zapper_workers,
                        chat,
                    )?),
                    Recipient::Zapper(reason) => Some(ZapperIssueDelivery {
                        chat_id: coordinator_chat.clone(),
                        state: ZapperIssueDeliveryState::Queued,
                        detail: Some(reason.into()),
                        reply: None,
                    }),
                    Recipient::None => None,
                };
            }
            // A person answering an issue that waits on them hands it back:
            // to the thread doing the work, else to the Zapper.
            if !coordinator && waiting {
                match delivery.as_ref() {
                    // The thread the answer went to takes the issue back.
                    Some(d)
                        if d.state == ZapperIssueDeliveryState::Queued
                            && d.chat_id != coordinator_chat =>
                    {
                        hand_back(issue, &zapper_workers, Some(&d.chat_id))
                    }
                    // Held from its thread: the Zapper owns it, never that
                    // stopped thread or another one picked in its place.
                    Some(d) if d.state == ZapperIssueDeliveryState::Held => {
                        issue.details.owner = Some(ZapperIssueOwner::Zapper);
                        issue.details.normalize(!issue.threads.is_empty());
                    }
                    // Left to the Zapper, or no routing: the existing contract.
                    _ => hand_back(issue, &zapper_workers, None),
                }
            }
            (issue_id, "comment", text.trim().to_owned())
        }
        ZapperAction::AskUser {
            issue_id,
            question,
            recommendation,
        } => {
            ensure!(
                !question.trim().is_empty() && question.len() <= 8192,
                "question must contain 1–8192 bytes"
            );
            let issue = find(zapper, issue_id)?;
            ensure!(
                !issue.details.status.terminal(),
                "reopen the issue before asking about it"
            );
            // The question belongs to the thread that held the issue, if it
            // still works for this Zapper; its answer goes straight back there.
            asked_for = match issue.details.current_owner() {
                Some(ZapperIssueOwner::Thread { chat_id }) if zapper_workers.contains(&chat_id) => {
                    Some(chat_id)
                }
                _ => None,
            };
            let (before_owner, before_status) =
                (issue.details.current_owner(), issue.details.status);
            issue.details.owner = Some(ZapperIssueOwner::User);
            issue.details.normalize(!issue.threads.is_empty());
            if before_owner != issue.details.current_owner() {
                changes.push(change(
                    "owner",
                    before_owner.as_ref().map(owner_value),
                    Some("user".into()),
                ));
            }
            if before_status != issue.details.status {
                changes.push(change(
                    "status",
                    Some(wire(&before_status)),
                    Some(wire(&issue.details.status)),
                ));
            }
            notify = !coordinator;
            let text = match recommendation.as_deref().map(str::trim) {
                Some(r) if !r.is_empty() => {
                    format!("{}\n\nRecommendation: {r}", question.trim())
                }
                _ => question.trim().to_owned(),
            };
            (issue_id, "question", text)
        }
        ZapperAction::CloseIssue {
            issue_id,
            outcome,
            note,
        } => {
            ensure!(
                !note.trim().is_empty() && note.len() <= 8192,
                "say what happened in the note (1–8192 bytes)"
            );
            let issue = find(zapper, issue_id)?;
            let (status, verb) = match outcome {
                ZapperIssueOutcome::Merged => {
                    ensure!(
                        !issue.pull_requests.is_empty(),
                        "link the merged pull request before closing the issue as merged"
                    );
                    (ZapperIssueStatus::Done, "Merged")
                }
                ZapperIssueOutcome::Dropped => (ZapperIssueStatus::Canceled, "Dropped"),
            };
            if issue.details.status != status {
                changes.push(change(
                    "status",
                    Some(wire(&issue.details.status)),
                    Some(wire(&status)),
                ));
            }
            issue.details.status = status;
            notify = !coordinator;
            (issue_id, "closed", format!("{verb} · {}", note.trim()))
        }
        ZapperAction::Delegate {
            issue_id: Some(issue_id),
            worker,
            ..
        } => {
            let issue = find(zapper, issue_id)?;
            ensure!(
                !issue.details.status.terminal(),
                "reopen the issue before starting work"
            );
            ensure!(
                issue.details.assigned,
                "assign the issue to this Zapper before starting work"
            );
            let (before_owner, before_status) =
                (issue.details.current_owner(), issue.details.status);
            issue.threads.push(worker.chat_id.clone());
            issue.details.owner = Some(ZapperIssueOwner::Thread {
                chat_id: worker.chat_id.clone(),
            });
            issue.details.status = ZapperIssueStatus::InProgress;
            issue.details.normalize(true);
            changes.push(change("thread", None, Some(worker.chat_id.clone())));
            changes.push(change(
                "owner",
                before_owner.as_ref().map(owner_value),
                Some(worker.chat_id.clone()),
            ));
            if before_status != ZapperIssueStatus::InProgress {
                changes.push(change(
                    "status",
                    Some(wire(&before_status)),
                    Some(wire(&issue.details.status)),
                ));
            }
            (
                issue_id,
                "thread",
                format!("Started thread {} · In progress", worker.chat_id),
            )
        }
        _ => return Ok(()),
    };
    let issue = find(zapper, id)?;
    issue.updated_at = now;
    let routed = delivery
        .as_ref()
        .filter(|d| d.chat_id != coordinator_chat)
        .map(|d| (d.chat_id.clone(), d.state, d.detail.clone()));
    if let Some((chat, ZapperIssueDeliveryState::Queued, _)) = &routed {
        let message = answer_message(issue, &text, reply.as_deref());
        tx.execute(
            "INSERT INTO zapper_outbox(id,zapper,chat,action) VALUES(?1,?2,?3,?4)",
            params![
                answer_id(&command.command_id),
                command.zapper_id,
                chat,
                serde_json::to_string(&ZapperAction::MessageWorker {
                    chat_id: chat.clone(),
                    text: message,
                    automatic: false,
                })?
            ],
        )?;
    }
    issue.activity.push(ZapperIssueActivity {
        actor: actor.map(str::to_owned),
        id: command.command_id.clone(),
        created_at: now,
        kind: kind.into(),
        text: text.clone(),
        reply_to: reply.clone(),
        delivery,
        thread: asked_for,
        changes,
    });
    if notify && !coordinator {
        // A reply carries what it answers, so the Zapper can act without a lookup.
        let context = reply
            .as_ref()
            .and_then(|parent| issue.activity.iter().find(|a| a.id == *parent))
            .map(|parent| {
                let quoted: String = parent.text.chars().take(280).collect();
                format!("\nIn reply to: {quoted}")
            })
            .unwrap_or_default();
        // Where the comment went rides the board's hidden `delivery`, not
        // this visible summary; the guide says not to relay a delivered one.
        let summary = format!(
            "{}: {}\n{}{}",
            issue.code(),
            issue.details.title,
            text,
            context
        );
        tx.execute(
            "INSERT INTO zapper_events(id,zapper,worker,kind,summary) VALUES(?1,?2,'','issueChanged',?3)",
            params![format!("issue-{}", command.command_id), zapper.id, summary],
        )?;
    }
    Ok(())
}
/// The Zapper as the coordinator prompt sees it. Issues carry only their recent
/// activity and a bounded description; storage keeps everything, and the
/// coordinator reads a full issue through `zapper_issue`.
pub(super) fn coordinator_view(zapper: &Zapper) -> Result<serde_json::Value> {
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
    let mut view = serde_json::to_value(zapper)?;
    // The purpose reaches the prompt as INSTRUCTIONS.md; once is enough.
    if let Some(config) = view["config"].as_object_mut() {
        config.remove("purpose");
    }
    view["issues"] = zapper
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
            if let Some(worker) = zapper.issue_queued_worker(issue) {
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
fn transition(
    issue: &ZapperIssue,
    next: &ZapperIssueDetails,
    child_in_progress: bool,
) -> Result<()> {
    let before = &issue.details;
    if next.status != before.status {
        ensure!(
            !next.status.terminal(),
            "close issues with closeIssue (outcome merged or dropped, plus a note)"
        );
        match next.status {
            ZapperIssueStatus::InReview => ensure!(
                !issue.pull_requests.is_empty(),
                "link the pull request before moving the issue to inReview"
            ),
            ZapperIssueStatus::InProgress => ensure!(
                !issue.threads.is_empty() || child_in_progress,
                "an issue is inProgress only while a thread works on it: delegate {{issueId: \"{}\"}} (which moves it to inProgress for you), or, for a parent issue, start a thread on one of its child issues (details.parent: \"{}\") first",
                issue.id,
                issue.id
            ),
            ZapperIssueStatus::NeedsYou => {
                bail!("hand the issue to the user with askUser")
            }
            _ => {}
        }
    }
    ensure!(
        next.current_owner() == before.current_owner()
            || next.current_owner() != Some(ZapperIssueOwner::User),
        "hand the issue to the user with askUser"
    );
    ensure!(
        before.done_criteria.trim().is_empty() || !next.done_criteria.trim().is_empty(),
        "keep the issue's doneCriteria; edit it rather than clearing it"
    );
    Ok(())
}
/// The user answered: the thread their answer went to takes the issue back,
/// else the newest linked thread still working for the Zapper, else the Zapper.
fn hand_back(issue: &mut ZapperIssue, workers: &[String], routed: Option<&str>) {
    let thread = routed.map(str::to_owned).or_else(|| {
        issue
            .threads
            .iter()
            .rev()
            .find(|id| workers.contains(id))
            .cloned()
    });
    issue.details.owner = Some(match thread {
        Some(chat_id) => ZapperIssueOwner::Thread { chat_id },
        None => ZapperIssueOwner::Zapper,
    });
    issue.details.normalize(!issue.threads.is_empty());
}
fn owner_label(owner: &ZapperIssueOwner) -> String {
    match owner {
        ZapperIssueOwner::Zapper => "Zapper".into(),
        ZapperIssueOwner::User => "You".into(),
        // Clients swap a thread's chat id for its title.
        ZapperIssueOwner::Thread { chat_id } => chat_id.clone(),
    }
}
/// An owner as a change value: `user`, `zapper`, or the thread's chat id.
fn owner_value(owner: &ZapperIssueOwner) -> String {
    match owner {
        ZapperIssueOwner::Zapper => "zapper".into(),
        ZapperIssueOwner::User => "user".into(),
        ZapperIssueOwner::Thread { chat_id } => chat_id.clone(),
    }
}
/// A status or priority by its wire name.
fn wire(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
fn change(field: &str, before: Option<String>, after: Option<String>) -> ZapperIssueChange {
    ZapperIssueChange {
        field: field.into(),
        before,
        after,
    }
}

/// Outbox and transcript message id of a person's comment sent to a thread.
const ANSWER: &str = ":answer";
pub(super) fn answer_id(comment: &str) -> String {
    format!("{comment}{ANSWER}")
}
/// Who a person's comment goes to.
#[derive(Debug, PartialEq, Eq)]
enum Recipient {
    /// One thread, named explicitly by what the person answered.
    Thread(String),
    /// The Zapper decides, and the comment says why.
    Zapper(&'static str),
    /// An ordinary comment: the Zapper hears about it as before.
    None,
}
/// A comment reaches a thread directly only when it answers that thread
/// explicitly: a reply to something the thread wrote, or an answer to the
/// open question asked for it. Nothing else is guessed.
fn recipient(
    issue: &ZapperIssue,
    target: Option<&ZapperIssueActivity>,
    waiting: bool,
    workers: &[String],
    expected: Option<u32>,
    changed: Option<u32>,
    coordinator: &str,
) -> Recipient {
    let question = |q: &ZapperIssueActivity| {
        let newest = issue.activity.iter().rev().find(|a| a.kind == "question");
        if !waiting {
            return Recipient::None;
        }
        if newest.map(|n| &n.id) != Some(&q.id) {
            return Recipient::Zapper(
                "A newer question replaced this one, so the Zapper routes your answer",
            );
        }
        match (&q.thread, expected) {
            (Some(_), None) => Recipient::Zapper(
                "Routing an answer to its thread needs the issue revision you answered, so the Zapper routes this one",
            ),
            (Some(chat), Some(e)) if changed.is_none_or(|c| e >= c) => {
                Recipient::Thread(chat.clone())
            }
            (Some(_), Some(_)) => Recipient::Zapper(
                "The issue changed after you opened it, so the Zapper routes your answer",
            ),
            (None, _) => Recipient::Zapper(
                "No single thread is tied to this question, so the Zapper routes your answer",
            ),
        }
    };
    match target {
        Some(t) => match t.actor.as_deref() {
            Some(actor)
                if actor != coordinator
                    && (t.kind == "workerReport"
                        || issue.threads.iter().any(|c| c == actor)
                        || workers.iter().any(|c| c == actor)) =>
            {
                Recipient::Thread(actor.to_owned())
            }
            _ if t.kind == "question" => question(t),
            _ => Recipient::None,
        },
        None if waiting => match issue.activity.iter().rev().find(|a| a.kind == "question") {
            Some(q) => question(q),
            None => Recipient::Zapper(
                "No single thread is tied to this question, so the Zapper routes your answer",
            ),
        },
        None => Recipient::None,
    }
}
/// Whether `chat` may receive a person's comment now: never a closed issue,
/// a stopped or archived Zapper, a thread that left the Zapper or the issue,
/// or a thread the person stopped. Held comments stay on the issue.
fn route(
    db: &Connection,
    zapper_id: &str,
    status: ZapperStatus,
    issue: &ZapperIssue,
    workers: &[String],
    chat: String,
) -> Result<ZapperIssueDelivery> {
    let reason = hold_reason(db, zapper_id, status, issue, workers, &chat)?;
    Ok(ZapperIssueDelivery {
        chat_id: chat,
        state: if reason.is_some() {
            ZapperIssueDeliveryState::Held
        } else {
            ZapperIssueDeliveryState::Queued
        },
        detail: reason.map(str::to_owned),
        reply: None,
    })
}
fn hold_reason(
    db: &Connection,
    zapper_id: &str,
    status: ZapperStatus,
    issue: &ZapperIssue,
    workers: &[String],
    chat: &str,
) -> Result<Option<&'static str>> {
    if issue.details.status.terminal() {
        return Ok(Some("This issue is closed"));
    }
    match status {
        ZapperStatus::Stopped => return Ok(Some("The Zapper is stopped")),
        ZapperStatus::Archived => return Ok(Some("The Zapper is archived")),
        ZapperStatus::Active | ZapperStatus::Paused => {}
    }
    if !workers.iter().any(|w| w == chat) {
        return Ok(Some("This thread no longer works for the Zapper"));
    }
    if !issue.threads.iter().any(|t| t == chat) {
        return Ok(Some("This thread is no longer linked to the issue"));
    }
    let hold: Option<String> = db
        .query_row(
            "SELECT reason FROM zapper_worker_holds WHERE zapper=?1 AND chat=?2",
            params![zapper_id, chat],
            |r| r.get(0),
        )
        .optional()?;
    if hold.as_deref().map(HoldSource::of) == Some(HoldSource::User) {
        return Ok(Some(
            "You stopped this thread. Continue it from its conversation first",
        ));
    }
    Ok(None)
}
/// The revision at which an issue's details last changed, when tracked.
fn issue_revision(db: &Connection, zapper: &str, issue: &str) -> Result<Option<u32>> {
    Ok(db
        .query_row(
            "SELECT revision FROM zapper_targets WHERE zapper=?1 AND target=?2",
            params![zapper, issue_target(issue)],
            |r| r.get(0),
        )
        .optional()?)
}
/// What the thread reads: who answered, where, and what they answered.
fn answer_message(issue: &ZapperIssue, text: &str, reply: Option<&str>) -> String {
    let quoted = reply
        .and_then(|id| issue.activity.iter().find(|a| a.id == id))
        .map(|a| {
            let quoted: String = a.text.chars().take(280).collect();
            format!("\n\nIn reply to: {quoted}")
        })
        .unwrap_or_default();
    format!(
        "The person replied on {} \"{}\":\n\n{}{}",
        issue.code(),
        issue.details.title,
        text.trim(),
        quoted
    )
}
/// Refresh routed comments from the outbox: queued, retrying, sent, or
/// answered once a report was correlated to the delivered message.
pub(super) fn mark_deliveries(db: &Connection, zapper: &mut Zapper) -> Result<()> {
    use std::collections::{HashMap, HashSet};
    let coordinators: Vec<String> = std::iter::once(zapper.coordinator_chat_id.clone())
        .chain(zapper.previous_coordinator_chats.iter().cloned())
        .collect();
    let routed = |d: &ZapperIssueDelivery| d.state != ZapperIssueDeliveryState::Held;
    let entries = || {
        zapper
            .issues
            .iter()
            .flat_map(|i| &i.activity)
            .filter(|a| a.delivery.as_ref().is_some_and(routed))
    };
    if entries().next().is_none() {
        return Ok(());
    }
    // A fixed number of queries per Zapper, whatever its history.
    let outbox: HashMap<String, (String, Option<String>)> = db
        .prepare_cached(
            "SELECT id,phase,error FROM zapper_outbox WHERE zapper=?1 AND id LIKE '%:answer'",
        )?
        .query_map([&zapper.id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
        .collect::<rusqlite::Result<_>>()?;
    let to_zapper: Vec<String> = entries()
        .filter(|a| {
            a.delivery
                .as_ref()
                .is_some_and(|d| coordinators.contains(&d.chat_id))
        })
        .map(|a| format!("issue-{}", a.id))
        .collect();
    let mut acknowledged: HashSet<String> = HashSet::new();
    let mut notified: HashSet<String> = HashSet::new();
    if !to_zapper.is_empty() {
        acknowledged = db
            .prepare_cached(
                "SELECT id FROM zapper_events WHERE zapper=?1 AND id LIKE 'issue-%' AND acknowledged<>0",
            )?
            .query_map([&zapper.id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let wanted: Vec<String> = to_zapper
            .iter()
            .flat_map(|event| {
                coordinators
                    .iter()
                    .map(move |chat| super::supervision::event_id(&[chat, event]))
            })
            .collect();
        for chunk in wanted.chunks(400) {
            let sql = format!(
                "SELECT id FROM zapper_notifications WHERE id IN ({})",
                vec!["?"; chunk.len()].join(",")
            );
            let mut stmt = db.prepare(&sql)?;
            for id in
                stmt.query_map(rusqlite::params_from_iter(chunk), |r| r.get::<_, String>(0))?
            {
                notified.insert(id?);
            }
        }
    }
    let paused = zapper.status == ZapperStatus::Paused;
    for issue in &mut zapper.issues {
        let ids: HashSet<String> = issue.activity.iter().map(|a| a.id.clone()).collect();
        for entry in &mut issue.activity {
            let Some(delivery) = entry.delivery.as_mut().filter(|d| routed(d)) else {
                continue;
            };
            if coordinators.contains(&delivery.chat_id) {
                // The Zapper hears through its event; it's sent once the
                // Zapper was woken with it or acknowledged it.
                let event = format!("issue-{}", entry.id);
                let woken = coordinators
                    .iter()
                    .any(|chat| notified.contains(&super::supervision::event_id(&[chat, &event])));
                if acknowledged.contains(&event) || woken {
                    delivery.state = ZapperIssueDeliveryState::Sent;
                } else if paused {
                    delivery.state = ZapperIssueDeliveryState::Queued;
                }
                continue;
            }
            let Some((phase, error)) = outbox.get(&answer_id(&entry.id)).cloned() else {
                continue;
            };
            (delivery.state, delivery.detail) = match phase.as_str() {
                "pending" => (
                    ZapperIssueDeliveryState::Queued,
                    paused.then(|| "Waiting for the Zapper to resume".to_owned()),
                ),
                "sending" if error.is_some() => (ZapperIssueDeliveryState::Retrying, error),
                "sending" => (ZapperIssueDeliveryState::Queued, None),
                "sent" | "settled" if delivery.reply.as_ref().is_some_and(|r| ids.contains(r)) => {
                    (ZapperIssueDeliveryState::Answered, None)
                }
                "sent" | "settled" => (ZapperIssueDeliveryState::Sent, None),
                _ => (
                    ZapperIssueDeliveryState::Cancelled,
                    Some(error.unwrap_or_else(|| "Stopped before it was sent".into())),
                ),
            };
        }
    }
    Ok(())
}
/// Recheck a routed comment just before it goes out: the issue, Zapper or
/// thread may have changed since it was queued.
pub(super) fn answer_hold(
    db: &Connection,
    zapper: &Zapper,
    outbox: &str,
) -> Result<Option<String>> {
    let Some(comment) = outbox.strip_suffix(ANSWER) else {
        return Ok(None);
    };
    let workers: Vec<String> = zapper.workers.iter().map(|w| w.chat_id.clone()).collect();
    for issue in &zapper.issues {
        if let Some(chat) = issue
            .activity
            .iter()
            .find(|a| a.id == comment)
            .and_then(|a| a.delivery.as_ref())
            .map(|d| d.chat_id.clone())
        {
            return Ok(
                hold_reason(db, &zapper.id, zapper.status, issue, &workers, &chat)?
                    .map(str::to_owned),
            );
        }
    }
    Ok(Some("The comment is no longer on the issue".into()))
}
/// The person's comment a finished turn answered: the transcript message
/// that started the turn ending at `turn`. `None` while the transcript
/// hasn't caught up to that turn; `Some(None)` when another message started it.
pub(super) fn answered_message(
    entries: &[zuse_model::SessionMessageEntry],
    turn: &str,
) -> Option<Option<String>> {
    let end = entries.iter().position(|e| e.id == turn)?;
    Some(
        entries[..end]
            .iter()
            .rev()
            .find(|e| e.role == zuse_model::MessageRole::User)
            .map(|e| e.id.clone())
            .filter(|id| id.ends_with(ANSWER)),
    )
}
impl Zappers {
    /// Whether `chat` was sent a routed comment that no report answers yet:
    /// only then is a finished turn worth matching against its transcript.
    pub(super) fn unanswered(&self, zapper: &str, chat: &str) -> Result<bool> {
        let db = self
            .inner
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?;
        let sent: Vec<String> = db
            .prepare_cached(
                "SELECT id FROM zapper_outbox WHERE zapper=?1 AND chat=?2 AND id LIKE '%:answer' AND phase IN ('sent','settled')",
            )?
            .query_map(params![zapper, chat], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if sent.is_empty() {
            return Ok(false);
        }
        let state: Option<String> = db
            .query_row("SELECT state FROM zappers WHERE id=?1", [zapper], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(state) = state else {
            return Ok(false);
        };
        let zapper: Zapper = serde_json::from_str(&state)?;
        Ok(zapper.issues.iter().flat_map(|i| &i.activity).any(|a| {
            sent.iter()
                .any(|id| id.strip_suffix(ANSWER) == Some(a.id.as_str()))
                && a.delivery.as_ref().is_some_and(|d| d.reply.is_none())
        }))
    }
    /// Match a finished turn to the routed comment whose message started it,
    /// by transcript identity alone. `Ok(false)`: it answers none.
    pub(super) fn correlate(
        &self,
        zapper: &str,
        chat: &str,
        report: &str,
        started: Option<&str>,
    ) -> Result<bool> {
        let Some(message) = started else {
            return Ok(false);
        };
        let delivered = self
            .inner
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?
            .query_row(
                "SELECT 1 FROM zapper_outbox WHERE id=?1 AND zapper=?2 AND chat=?3 AND phase IN ('sent','settled')",
                params![message, zapper, chat],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !delivered {
            return Ok(false);
        }
        self.link_answer(zapper, chat, report, message)?;
        Ok(true)
    }
    /// Record that `report` answers the comment delivered as `message`, and
    /// nest the report under it if it's already posted. A recorded answer is
    /// never replaced, so a reread or replay can't move a reply.
    pub(super) fn link_answer(
        &self,
        zapper_id: &str,
        chat: &str,
        report: &str,
        message: &str,
    ) -> Result<()> {
        let comment = message
            .strip_suffix(ANSWER)
            .context("not a routed comment")?;
        let mut db = self
            .inner
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?;
        let tx = db.transaction()?;
        let delivered = tx
            .query_row(
                "SELECT 1 FROM zapper_outbox WHERE id=?1 AND zapper=?2 AND chat=?3 AND phase IN ('sent','settled')",
                params![message, zapper_id, chat],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        ensure!(delivered, "that comment was not delivered to this thread");
        let state: String =
            tx.query_row("SELECT state FROM zappers WHERE id=?1", [zapper_id], |r| {
                r.get(0)
            })?;
        let mut zapper: Zapper = serde_json::from_str(&state)?;
        let before = zapper.clone();
        for issue in &mut zapper.issues {
            let Some(entry) = issue.activity.iter_mut().find(|a| a.id == comment) else {
                continue;
            };
            let root = entry.reply_to.clone().unwrap_or_else(|| entry.id.clone());
            let Some(delivery) = entry.delivery.as_mut().filter(|d| d.chat_id == chat) else {
                continue;
            };
            if delivery.reply.is_some() {
                continue;
            }
            delivery.reply = Some(report.to_owned());
            if let Some(posted) = issue.activity.iter_mut().find(|a| {
                a.id == report && a.actor.as_deref() == Some(chat) && a.reply_to.is_none()
            }) {
                posted.reply_to = Some(root);
            }
        }
        let changed = zapper != before;
        if changed {
            zapper.revision = zapper
                .revision
                .checked_add(1)
                .context("Zapper revision exhausted")?;
            record_changes(&tx, &before, &zapper)?;
            tx.execute(
                "UPDATE zappers SET state=?2 WHERE id=?1",
                params![zapper.id, serde_json::to_string(&zapper)?],
            )?;
        }
        tx.commit()?;
        if changed {
            self.inner.changed.send_modify(|n| *n = n.wrapping_add(1));
        }
        Ok(())
    }
}
/// Turns already found to answer no routed comment, so the transcript isn't
/// reread every pass. Memory only: after a restart a turn is checked once more.
static CHECKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
pub(super) fn checked_turn(report: &str) -> bool {
    CHECKED
        .lock()
        .is_ok_and(|seen| seen.iter().any(|r| r == report))
}
pub(super) fn remember_turn(report: &str) {
    if let Ok(mut seen) = CHECKED.lock() {
        if seen.len() >= 1024 {
            seen.remove(0);
        }
        seen.push(report.to_owned());
    }
}
fn find<'a>(zapper: &'a mut Zapper, id: &str) -> Result<&'a mut ZapperIssue> {
    zapper
        .issues
        .iter_mut()
        .find(|i| i.id == id)
        .context("issue not found")
}
fn validate(zapper: &Zapper, id: &str, details: &ZapperIssueDetails) -> Result<()> {
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
        .filter(|r| !zapper.issues.iter().any(|i| i.id == **r))
        .map(String::as_str)
        .collect();
    ensure!(
        unknown.is_empty(),
        "relations must reference existing issues in this Zapper; unknown: {}. Create those issues first (createIssue, one command each), then add the relation with editIssue, or leave relations empty for now",
        unknown.join(", ")
    );
    if let Some(ZapperIssueOwner::Thread { chat_id }) = &details.owner {
        ensure!(
            zapper.workers.iter().any(|w| w.chat_id == *chat_id),
            "an owning thread must belong to this Zapper"
        );
    }
    if let Some(parent) = &details.parent {
        ensure!(parent != id, "an issue can't be its own parent");
        ensure!(
            zapper.issues.iter().any(|i| i.id == *parent),
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
pub(super) fn report(tx: &rusqlite::Transaction<'_>, event: &ZapperEvent) -> Result<()> {
    let state: String = tx.query_row(
        "SELECT state FROM zappers WHERE id=?1",
        [&event.zapper_id],
        |r| r.get(0),
    )?;
    let mut zapper: Zapper = serde_json::from_str(&state)?;
    let before = zapper.clone();
    let mut changed = false;
    for issue in &mut zapper.issues {
        if !issue.threads.contains(&event.worker_chat_id) {
            continue;
        }
        // Nested under the routed comment whose message started this turn.
        let reply_to = issue
            .activity
            .iter()
            .find(|a| {
                a.delivery.as_ref().is_some_and(|d| {
                    d.chat_id == event.worker_chat_id
                        && d.reply.as_deref() == Some(event.id.as_str())
                })
            })
            .map(|a| a.reply_to.clone().unwrap_or_else(|| a.id.clone()));
        let now = chrono::Utc::now().timestamp_millis();
        issue.activity.push(ZapperIssueActivity {
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
            reply_to,
            ..Default::default()
        });
        issue.updated_at = now;
        let reported = report_status(&event.summary);
        if !issue.details.status.terminal()
            && issue.details.status != ZapperIssueStatus::NeedsYou
            && (matches!(
                event.kind.as_str(),
                "failed" | "interrupted" | "needsInput" | "bindingPending"
            ) || matches!(reported, Some(ReportStatus::Blocked)))
        {
            // A blocked thread hands the issue back to the Zapper to unblock.
            issue.details.status = ZapperIssueStatus::Blocked;
            issue.details.owner = Some(ZapperIssueOwner::Zapper);
            issue.details.normalize(true);
        }
        changed = true;
    }
    if changed {
        zapper.revision = zapper
            .revision
            .checked_add(1)
            .context("Zapper revision exhausted")?;
        record_changes(tx, &before, &zapper)?;
        tx.execute(
            "UPDATE zappers SET state=?2 WHERE id=?1",
            params![zapper.id, serde_json::to_string(&zapper)?],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zappers::tests::{command, config, create, worker};
    fn details() -> ZapperIssueDetails {
        ZapperIssueDetails {
            title: "Ship the fix".into(),
            description: "Pass acceptance".into(),
            assigned: true,
            ..Default::default()
        }
    }
    fn issue(store: &Zappers) -> ZapperIssue {
        store.get("release").unwrap().issues[0].clone()
    }
    fn add(store: &Zappers) {
        store
            .command(command(
                "release",
                "issue",
                ZapperAction::CreateIssue {
                    issue_id: "fix".into(),
                    details: details(),
                },
            ))
            .unwrap();
    }
    #[test]
    fn coordinator_view_keeps_recent_activity_and_flags_clipped_descriptions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let mut long = details();
        long.description = "x".repeat(5000);
        store
            .command(command(
                "release",
                "long",
                ZapperAction::EditIssue {
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
                    ZapperAction::CommentIssue {
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
                ZapperAction::CommentIssue {
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
                ZapperAction::CommentIssue {
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
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        store
            .command(command(
                "release",
                "adopt",
                ZapperAction::AdoptWorker { worker: worker() },
            ))
            .unwrap();
        store
            .command(command(
                "release",
                "link",
                ZapperAction::LinkIssueThread {
                    issue_id: "fix".into(),
                    chat_id: "worker".into(),
                },
            ))
            .unwrap();
        store
            .command(command(
                "release",
                "pr",
                ZapperAction::LinkIssuePullRequest {
                    issue_id: "fix".into(),
                    url: "https://github.com/team/repo/pull/123".into(),
                    title: "Fix".into(),
                },
            ))
            .unwrap();
        let unlink = command(
            "release",
            "unlink",
            ZapperAction::UnlinkIssueThread {
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
                ZapperAction::UnlinkIssuePullRequest {
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
                ZapperAction::UnlinkIssueThread {
                    issue_id: "fix".into(),
                    chat_id: "worker".into(),
                },
            ))
            .unwrap();
        assert_eq!(issue(&store).activity.len(), activity);
    }
    fn as_zapper(store: &Zappers, id: &str, action: ZapperAction) -> Result<Option<Zapper>> {
        store.command_as(command("release", id, action), Some("Release"), true)
    }
    #[test]
    fn coordinator_status_moves_need_their_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut todo = details();
        todo.status = ZapperIssueStatus::Todo;
        let create_fix = |id: &str, details: ZapperIssueDetails| {
            as_zapper(
                &store,
                id,
                ZapperAction::CreateIssue {
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
            Some(ZapperIssueOwner::Zapper)
        );
        let edit = |id: &str, status: ZapperIssueStatus| {
            let mut next = issue(&store).details;
            next.status = status;
            as_zapper(
                &store,
                id,
                ZapperAction::EditIssue {
                    issue_id: "fix".into(),
                    details: next,
                },
            )
        };
        for (status, expected) in [
            (ZapperIssueStatus::InReview, "link the pull request"),
            (ZapperIssueStatus::InProgress, "start a thread"),
            (ZapperIssueStatus::Done, "closeIssue"),
            (ZapperIssueStatus::NeedsYou, "askUser"),
        ] {
            let err = edit(&format!("to-{status:?}"), status).unwrap_err();
            assert!(err.to_string().contains(expected), "{status:?}: {err}");
        }
        let err = as_zapper(
            &store,
            "merge-without-pr",
            ZapperAction::CloseIssue {
                issue_id: "fix".into(),
                outcome: ZapperIssueOutcome::Merged,
                note: "Shipped".into(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("link the merged pull request"));
        as_zapper(
            &store,
            "drop",
            ZapperAction::CloseIssue {
                issue_id: "fix".into(),
                outcome: ZapperIssueOutcome::Dropped,
                note: "Superseded by the new login flow".into(),
            },
        )
        .unwrap();
        let closed = issue(&store);
        assert_eq!(closed.details.status, ZapperIssueStatus::Canceled);
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
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut parent = details();
        parent.status = ZapperIssueStatus::Todo;
        parent.done_criteria = "Both parts ship".into();
        as_zapper(
            &store,
            "parent",
            ZapperAction::CreateIssue {
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
            ZapperAction::CreateIssue {
                issue_id: "child".into(),
                details: child,
            },
        )
        .unwrap();
        let mut moving = parent.clone();
        moving.status = ZapperIssueStatus::InProgress;
        let move_parent = |id: &str| {
            as_zapper(
                &store,
                id,
                ZapperAction::EditIssue {
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
            "zapper-worker-start",
            ZapperAction::Delegate {
                issue_id: Some("child".into()),
                worker: ZapperWorker {
                    chat_id: "zapper-worker-start".into(),
                    ..worker()
                },
                model: config().coordinator_model,
                base: "origin/main".into(),
            },
        )
        .unwrap();
        move_parent("with-child").unwrap();
        let zapper = store.get("release").unwrap();
        let parent = zapper.issues.iter().find(|i| i.id == "parent").unwrap();
        assert_eq!(parent.details.status, ZapperIssueStatus::InProgress);
    }
    #[test]
    fn delegating_records_the_owner_it_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut todo = details();
        todo.status = ZapperIssueStatus::Todo;
        todo.done_criteria = "It ships".into();
        as_zapper(
            &store,
            "fix",
            ZapperAction::CreateIssue {
                issue_id: "fix".into(),
                details: todo,
            },
        )
        .unwrap();
        as_zapper(
            &store,
            "zapper-worker-start",
            ZapperAction::Delegate {
                issue_id: Some("fix".into()),
                worker: ZapperWorker {
                    chat_id: "zapper-worker-start".into(),
                    ..worker()
                },
                model: config().coordinator_model,
                base: "origin/main".into(),
            },
        )
        .unwrap();
        let zapper = store.get("release").unwrap();
        let started = zapper.issues[0]
            .activity
            .iter()
            .find(|a| a.id == "zapper-worker-start")
            .unwrap();
        assert!(
            started.changes.contains(&change(
                "owner",
                Some("zapper".into()),
                Some("zapper-worker-start".into())
            )),
            "{:?}",
            started.changes
        );
    }
    #[test]
    fn unknown_relations_and_parents_say_to_create_them_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut related = details();
        related.relations = vec!["later".into()];
        let err = store
            .command(command(
                "release",
                "related",
                ZapperAction::CreateIssue {
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
                ZapperAction::CreateIssue {
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
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        store
            .command(command(
                "release",
                "adopt",
                ZapperAction::AdoptWorker { worker: worker() },
            ))
            .unwrap();
        store
            .command(command(
                "release",
                "link",
                ZapperAction::LinkIssueThread {
                    issue_id: "fix".into(),
                    chat_id: "worker".into(),
                },
            ))
            .unwrap();
        let before = store.events("release").unwrap().len();
        as_zapper(
            &store,
            "ask",
            ZapperAction::AskUser {
                issue_id: "fix".into(),
                question: "Return 401 or 403 for a failed login?".into(),
                recommendation: Some("401, matching the API".into()),
            },
        )
        .unwrap();
        let asked = issue(&store);
        assert_eq!(asked.details.current_owner(), Some(ZapperIssueOwner::User));
        assert_eq!(asked.details.status, ZapperIssueStatus::NeedsYou);
        assert!(!asked.details.assigned);
        assert_eq!(asked.activity.last().unwrap().kind, "question");
        assert_eq!(store.events("release").unwrap().len(), before);
        store
            .command(command(
                "release",
                "answer",
                ZapperAction::CommentIssue {
                    issue_id: "fix".into(),
                    text: "401".into(),
                    reply_to: None,
                },
            ))
            .unwrap();
        let answered = issue(&store);
        assert_eq!(
            answered.details.current_owner(),
            Some(ZapperIssueOwner::Thread {
                chat_id: "worker".into()
            })
        );
        assert_eq!(answered.details.status, ZapperIssueStatus::InProgress);
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
        let legacy: ZapperIssueDetails = serde_json::from_str(
            r#"{"title":"Old","description":"","assigned":true,"dueDate":null}"#,
        )
        .unwrap();
        assert_eq!(legacy.current_owner(), Some(ZapperIssueOwner::Zapper));
        assert!(legacy.done_criteria.is_empty());
    }
    #[test]
    fn coordinator_mutations_do_not_create_self_notification_loops() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        store
            .command_as(
                command(
                    "release",
                    "self-issue",
                    ZapperAction::CreateIssue {
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
            ZapperAction::CommentIssue {
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
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let rev = store.get("release").unwrap().revision;
        let mut changed = details();
        changed.status = ZapperIssueStatus::InReview;
        let mut edit = command(
            "release",
            "edit",
            ZapperAction::EditIssue {
                issue_id: "fix".into(),
                details: changed.clone(),
            },
        );
        edit.expected_revision = Some(rev);
        let result = store.command(edit.clone()).unwrap();
        let mut stale = edit.clone();
        stale.command_id = "stale".into();
        assert!(store.command(stale).is_err());
        assert_eq!(issue(&store).details.status, ZapperIssueStatus::InReview);
        drop(store);
        let store = Zappers::open(dir.path(), "local").unwrap();
        assert_eq!(store.command(edit).unwrap(), result);
        assert_eq!(issue(&store).number, 1);
        assert_eq!(issue(&store).activity.len(), 2);
        assert_eq!(store.events("release").unwrap().len(), 2);
    }
    #[test]
    fn delegation_links_atomically_and_worker_reports_never_finish_issues() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let delegate = command(
            "release",
            "delegate",
            ZapperAction::Delegate {
                issue_id: Some("fix".into()),
                worker: worker(),
                model: config().coordinator_model,
                base: "main".into(),
            },
        );
        store.command(delegate.clone()).unwrap();
        store.command(delegate).unwrap();
        assert_eq!(issue(&store).threads, vec!["worker"]);
        assert_eq!(issue(&store).details.status, ZapperIssueStatus::InProgress);
        assert_eq!(store.outbox().unwrap().len(), 1);
        let mut event = ZapperEvent {
            id: "report".into(),
            zapper_id: "release".into(),
            worker_chat_id: "worker".into(),
            kind: "settled".into(),
            summary: "Tests passed".into(),
            acknowledged: false,
        };
        store.record_event(&event).unwrap();
        store.record_event(&event).unwrap();
        assert_eq!(issue(&store).activity.len(), 3);
        assert_eq!(issue(&store).details.status, ZapperIssueStatus::InProgress);
        event.id = "failure".into();
        event.kind = "failed".into();
        store.record_event(&event).unwrap();
        assert_eq!(issue(&store).details.status, ZapperIssueStatus::Blocked);
    }
    #[test]
    fn invalid_links_and_delegations_do_not_partially_mutate_issues() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        add(&store);
        let before = store.get("release").unwrap();
        assert!(
            store
                .command(command(
                    "release",
                    "bad-thread",
                    ZapperAction::LinkIssueThread {
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
                        ZapperAction::LinkIssuePullRequest {
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
                    ZapperAction::Delegate {
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
                ZapperAction::LinkIssuePullRequest {
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
    fn old_zappers_default_to_no_issues_and_relations_stay_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let mut json = serde_json::to_value(store.get("release").unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("issues");
        assert!(
            serde_json::from_value::<Zapper>(json)
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
                    ZapperAction::CreateIssue {
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
                    ZapperAction::EditIssue {
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
    use crate::zappers::tests::{command, create, worker};
    #[test]
    fn activity_notices_keep_host_instructions_out_of_the_visible_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let zapper = store.get("release").unwrap();
        let mut request: zuse_protocol::RunRequest = serde_json::from_value(serde_json::json!({
            "prompt": zuse_protocol::presentation::zapper_message::activity_from(&zapper, "Implementation thread", "Activity update\nVerification passed; ready for review."),
            "cwd": "/tmp", "sandbox": "workspace-write"
        })).unwrap();
        store
            .prepare_request(
                &zapper.coordinator_chat_id,
                zuse_protocol::HarnessId::ClaudeCode,
                &mut request,
            )
            .unwrap();
        let (sender, visible) =
            zuse_protocol::presentation::zapper_message::parse(&request.prompt).unwrap();
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
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        store
            .command(command(
                "release",
                "adopt",
                ZapperAction::AdoptWorker { worker: worker() },
            ))
            .unwrap();
        let zapper = store.get("release").unwrap();
        let mut request: zuse_protocol::RunRequest=serde_json::from_value(serde_json::json!({"prompt":zuse_protocol::presentation::zapper_message::envelope(&zapper,"Implement the issue"),"cwd":"/tmp","sandbox":"workspace-write"})).unwrap();
        store
            .prepare_request("worker", zuse_protocol::HarnessId::ClaudeCode, &mut request)
            .unwrap();
        let (sender, display) =
            zuse_protocol::presentation::zapper_message::parse(&request.prompt).unwrap();
        assert_eq!(sender.name, "Release");
        assert_eq!(display, "Implement the issue");
        assert!(
            request
                .prompt
                .contains("You are a thread working for the Zapper \"Release\" (zapperId release")
        );
        assert!(
            !request
                .prompt
                .contains(zuse_protocol::zappers::legacy::ID_FIELD)
        );
        let once = request.prompt.clone();
        store
            .prepare_request("worker", zuse_protocol::HarnessId::ClaudeCode, &mut request)
            .unwrap();
        assert_eq!(request.prompt, once);
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use crate::zappers::tests::{command, create, worker};
    use zuse_model::{MessagePart, MessageRole, SessionMessageEntry};

    fn store() -> (tempfile::TempDir, Zappers) {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        (dir, store)
    }
    fn run(store: &Zappers, id: &str, action: ZapperAction) -> Zapper {
        store
            .command(command("release", id, action))
            .unwrap()
            .unwrap()
    }
    fn issue(store: &Zappers) -> ZapperIssue {
        store.get("release").unwrap().issues[0].clone()
    }
    fn entry(store: &Zappers, id: &str) -> ZapperIssueActivity {
        issue(store)
            .activity
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
    }
    /// An issue with two linked threads, each of which has reported once.
    fn board(store: &Zappers) {
        run(
            store,
            "issue",
            ZapperAction::CreateIssue {
                issue_id: "fix".into(),
                details: ZapperIssueDetails {
                    title: "Ship the fix".into(),
                    description: String::new(),
                    assigned: true,
                    ..Default::default()
                },
            },
        );
        for chat in ["worker", "other"] {
            run(
                store,
                &format!("adopt-{chat}"),
                ZapperAction::AdoptWorker {
                    worker: ZapperWorker {
                        chat_id: chat.into(),
                        ..worker()
                    },
                },
            );
            run(
                store,
                &format!("link-{chat}"),
                ZapperAction::LinkIssueThread {
                    issue_id: "fix".into(),
                    chat_id: chat.into(),
                },
            );
            store
                .record_event(&ZapperEvent {
                    id: format!("report-{chat}"),
                    zapper_id: "release".into(),
                    worker_chat_id: chat.into(),
                    kind: "settled".into(),
                    summary: format!("{chat}: A or B?"),
                    acknowledged: false,
                })
                .unwrap();
        }
    }
    fn reply(id: &str, to: &str) -> ZapperCommand {
        let mut command = command(
            "release",
            id,
            ZapperAction::CommentIssue {
                issue_id: "fix".into(),
                text: "B".into(),
                reply_to: Some(to.into()),
            },
        );
        command.expected_revision = None;
        command
    }
    fn answers(store: &Zappers) -> Vec<(String, String)> {
        store
            .outbox()
            .unwrap()
            .into_iter()
            .filter(|(id, ..)| id.ends_with(ANSWER))
            .map(|(id, _, chat, ..)| (id, chat))
            .collect()
    }
    fn delivery(store: &Zappers, id: &str) -> ZapperIssueDelivery {
        entry(store, id).delivery.unwrap()
    }

    #[test]
    fn a_reply_to_a_thread_reaches_exactly_that_thread_once() {
        let (dir, store) = store();
        board(&store);
        let answer = reply("answer", "report-other");
        store.command(answer.clone()).unwrap();
        store.command(answer.clone()).unwrap();
        assert_eq!(
            answers(&store),
            vec![("answer:answer".into(), "other".into())]
        );
        let routed = delivery(&store, "answer");
        assert_eq!(routed.chat_id, "other");
        assert_eq!(routed.state, ZapperIssueDeliveryState::Queued);
        // The thread reads what it was asked and the answer.
        let (_, _, _, action, _) = store.outbox().unwrap().pop().unwrap();
        let ZapperAction::MessageWorker {
            text, automatic, ..
        } = action
        else {
            panic!("not a message");
        };
        assert!(!automatic);
        assert!(
            text.ends_with(":\n\nB\n\nIn reply to: Worker update\nother: A or B?"),
            "{text}"
        );
        // The Zapper hears the comment; where it went stays in the board's
        // delivery metadata, out of the visible summary.
        let event = store
            .events("release")
            .unwrap()
            .into_iter()
            .find(|e| e.id == "issue-answer")
            .unwrap();
        assert!(
            !event.summary.contains("answer:answer"),
            "{}",
            event.summary
        );
        let view = coordinator_view(&store.get("release").unwrap()).unwrap();
        let recent = view["issues"][0]["recentActivity"].as_array().unwrap();
        assert_eq!(recent.last().unwrap()["delivery"]["chatId"], "other");
        // A restart replays the receipt, never a second delivery.
        drop(store);
        let store = Zappers::open(dir.path(), "local").unwrap();
        store.command(answer).unwrap();
        assert_eq!(answers(&store).len(), 1);
        // The worker's objective is untouched by an answer.
        assert_eq!(
            store.get("release").unwrap().workers[1].objective,
            worker().objective
        );
    }

    #[test]
    fn workers_and_the_zapper_never_route_comments_to_threads() {
        let (_dir, store) = store();
        board(&store);
        store
            .command_as(reply("from-worker", "report-other"), Some("worker"), false)
            .unwrap();
        store
            .command_as(reply("from-zapper", "report-other"), Some("Release"), true)
            .unwrap();
        assert!(answers(&store).is_empty());
        assert!(entry(&store, "from-worker").delivery.is_none());
        assert!(entry(&store, "from-zapper").delivery.is_none());
    }

    #[test]
    fn ordinary_comments_stay_with_the_zapper() {
        let (_dir, store) = store();
        board(&store);
        run(
            &store,
            "note",
            ZapperAction::CommentIssue {
                issue_id: "fix".into(),
                text: "FYI the API moved".into(),
                reply_to: None,
            },
        );
        assert!(answers(&store).is_empty());
        assert!(entry(&store, "note").delivery.is_none());
        assert!(
            store
                .events("release")
                .unwrap()
                .iter()
                .any(|e| e.id == "issue-note")
        );
    }

    fn ask(store: &Zappers, owner: Option<&str>) {
        if let Some(chat) = owner {
            let mut details = issue(store).details;
            details.owner = Some(ZapperIssueOwner::Thread {
                chat_id: chat.into(),
            });
            run(
                store,
                "own",
                ZapperAction::EditIssue {
                    issue_id: "fix".into(),
                    details,
                },
            );
        }
        store
            .command_as(
                command(
                    "release",
                    "ask",
                    ZapperAction::AskUser {
                        issue_id: "fix".into(),
                        question: "A or B?".into(),
                        recommendation: None,
                    },
                ),
                Some("Release"),
                true,
            )
            .unwrap();
    }
    fn answer(store: &Zappers, id: &str, revision: Option<u32>) {
        let mut answer = command(
            "release",
            id,
            ZapperAction::CommentIssue {
                issue_id: "fix".into(),
                text: "B".into(),
                reply_to: None,
            },
        );
        answer.expected_revision = revision;
        store.command(answer).unwrap();
    }

    #[test]
    fn an_answer_goes_to_the_thread_its_question_was_asked_for() {
        let (_dir, store) = store();
        board(&store);
        ask(&store, Some("worker"));
        assert_eq!(entry(&store, "ask").thread.as_deref(), Some("worker"));
        let revision = store.get("release").unwrap().revision;
        answer(&store, "answer", Some(revision));
        assert_eq!(
            answers(&store),
            vec![("answer:answer".into(), "worker".into())]
        );
        assert_eq!(
            issue(&store).details.current_owner(),
            Some(ZapperIssueOwner::Thread {
                chat_id: "worker".into()
            })
        );
    }

    #[test]
    fn untied_or_stale_answers_fall_back_to_the_zapper_with_a_reason() {
        for (case, owner, revision) in [
            ("untied", None, Some(true)),
            ("stale", Some("worker"), Some(false)),
            ("unversioned", Some("worker"), None),
        ] {
            let (_dir, store) = store();
            board(&store);
            ask(&store, owner);
            let current = store.get("release").unwrap().revision;
            answer(
                &store,
                "answer",
                revision.map(|fresh| if fresh { current } else { current - 3 }),
            );
            assert!(answers(&store).is_empty(), "{case}");
            let routed = delivery(&store, "answer");
            assert_eq!(
                routed.chat_id,
                store.get("release").unwrap().coordinator_chat_id,
                "{case}"
            );
            let detail = routed.detail.unwrap();
            assert!(detail.contains("Zapper routes"), "{case}");
            // A caller that sends no revision is told exactly that.
            assert_eq!(
                case == "unversioned",
                detail.contains("revision you answered"),
                "{case}: {detail}"
            );
        }
    }

    #[test]
    fn closed_stopped_and_released_work_is_never_revived() {
        type Setup = fn(&Zappers);
        let cases: [(&str, Setup, &str); 4] = [
            (
                "user stopped",
                |s| {
                    run(
                        s,
                        "stop",
                        ZapperAction::StopWorker {
                            chat_id: "other".into(),
                        },
                    );
                },
                "You stopped this thread",
            ),
            (
                "closed",
                |s| {
                    let mut details = issue(s).details;
                    details.status = ZapperIssueStatus::Done;
                    run(
                        s,
                        "close",
                        ZapperAction::EditIssue {
                            issue_id: "fix".into(),
                            details,
                        },
                    );
                },
                "closed",
            ),
            (
                "released",
                |s| {
                    run(
                        s,
                        "release",
                        ZapperAction::ReleaseWorker {
                            chat_id: "other".into(),
                        },
                    );
                },
                "no longer works",
            ),
            (
                "zapper stopped",
                |s| {
                    run(
                        s,
                        "halt",
                        ZapperAction::SetStatus {
                            status: ZapperStatus::Stopped,
                        },
                    );
                },
                "Zapper is stopped",
            ),
        ];
        for (case, setup, reason) in cases {
            let (_dir, store) = store();
            board(&store);
            setup(&store);
            store.command(reply("answer", "report-other")).unwrap();
            assert!(answers(&store).is_empty(), "{case}");
            let held = delivery(&store, "answer");
            assert_eq!(held.state, ZapperIssueDeliveryState::Held, "{case}");
            assert!(held.detail.unwrap().contains(reason), "{case}");
            let event = store
                .events("release")
                .unwrap()
                .into_iter()
                .find(|e| e.id == "issue-answer")
                .unwrap();
            assert!(!event.summary.contains("Not delivered"), "{case}");
        }
    }

    #[test]
    fn queued_answers_wait_while_paused_and_are_rechecked_before_sending() {
        let (_dir, store) = store();
        board(&store);
        run(
            &store,
            "pause",
            ZapperAction::SetStatus {
                status: ZapperStatus::Paused,
            },
        );
        store.command(reply("answer", "report-other")).unwrap();
        let queued = delivery(&store, "answer");
        assert_eq!(queued.state, ZapperIssueDeliveryState::Queued);
        assert_eq!(
            queued.detail.as_deref(),
            Some("Waiting for the Zapper to resume")
        );
        let zapper = store.get("release").unwrap();
        let db = store.inner.db.lock().unwrap();
        assert_eq!(answer_hold(&db, &zapper, "answer:answer").unwrap(), None);
        drop(db);
        // Closing the issue after it was queued stops it at dispatch.
        let mut details = issue(&store).details;
        details.status = ZapperIssueStatus::Canceled;
        run(
            &store,
            "drop",
            ZapperAction::EditIssue {
                issue_id: "fix".into(),
                details,
            },
        );
        let zapper = store.get("release").unwrap();
        let db = store.inner.db.lock().unwrap();
        assert_eq!(
            answer_hold(&db, &zapper, "answer:answer")
                .unwrap()
                .as_deref(),
            Some("This issue is closed")
        );
        drop(db);
        store
            .outbox_phase("answer:answer", "cancelled", Some("This issue is closed"))
            .unwrap();
        let cancelled = delivery(&store, "answer");
        assert_eq!(cancelled.state, ZapperIssueDeliveryState::Cancelled);
        assert_eq!(cancelled.detail.as_deref(), Some("This issue is closed"));
    }

    fn sent(store: &Zappers, id: &str, baseline: &str) {
        store.outbox_phase(id, "sent", None).unwrap();
        store
            .inner
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT OR IGNORE INTO zapper_delivery VALUES(?1,?2,1)",
                params![id, baseline],
            )
            .unwrap();
    }
    fn report(store: &Zappers, id: &str) {
        store
            .record_event(&ZapperEvent {
                id: id.into(),
                zapper_id: "release".into(),
                worker_chat_id: "other".into(),
                kind: "settled".into(),
                summary: "B it is.".into(),
                acknowledged: false,
            })
            .unwrap();
    }

    #[test]
    fn a_report_nests_only_under_the_comment_that_started_its_turn() {
        let (_dir, store) = store();
        board(&store);
        store.command(reply("answer", "report-other")).unwrap();
        // Accepted by the thread's queue: sent, not answered.
        sent(&store, "answer:answer", "turn-0");
        assert_eq!(
            delivery(&store, "answer").state,
            ZapperIssueDeliveryState::Sent
        );
        assert!(store.unanswered("release", "other").unwrap());
        // Another message started this turn: it answers nothing.
        assert!(
            !store
                .correlate("release", "other", "report-1", Some("typed"))
                .unwrap()
        );
        assert!(
            !store
                .correlate("release", "other", "report-1", None)
                .unwrap()
        );
        // Linked before the report is recorded, as reconcile does.
        assert!(
            store
                .correlate("release", "other", "report-2", Some("answer:answer"))
                .unwrap()
        );
        report(&store, "report-2");
        // Replies stay one level deep: it joins the card the comment is in.
        assert_eq!(
            entry(&store, "report-2").reply_to.as_deref(),
            Some("report-other")
        );
        assert_eq!(
            delivery(&store, "answer").state,
            ZapperIssueDeliveryState::Answered
        );
        assert!(!store.unanswered("release", "other").unwrap());
        // A replayed link or a later report never moves the reply.
        store
            .correlate("release", "other", "report-3", Some("answer:answer"))
            .unwrap();
        report(&store, "report-3");
        assert_eq!(entry(&store, "report-3").reply_to, None);
        assert_eq!(
            delivery(&store, "answer").reply.as_deref(),
            Some("report-2")
        );
        // Only the thread it went to can be answered from.
        assert!(
            !store
                .correlate("release", "worker", "report-x", Some("answer:answer"))
                .unwrap()
        );
    }

    #[test]
    fn queued_answers_each_match_their_own_turn_across_a_restart() {
        use MessageRole::{Assistant, User};
        let (dir, store) = store();
        board(&store);
        store.command(reply("a", "report-other")).unwrap();
        store.command(reply("b", "report-other")).unwrap();
        // A went out and its turn finished; B waited behind it.
        sent(&store, "a:answer", "turn-0");
        let transcript = vec![message("a:answer", User), message("turn-a", Assistant)];
        let started = answered_message(&transcript, "turn-a").unwrap();
        assert!(
            store
                .correlate("release", "other", "report-a", started.as_deref())
                .unwrap()
        );
        report(&store, "report-a");
        assert_eq!(
            delivery(&store, "a").state,
            ZapperIssueDeliveryState::Answered
        );
        assert_eq!(
            delivery(&store, "b").state,
            ZapperIssueDeliveryState::Queued
        );
        // B goes out; a restart before its turn finishes changes nothing.
        sent(&store, "b:answer", "turn-a");
        assert_eq!(delivery(&store, "b").state, ZapperIssueDeliveryState::Sent);
        drop(store);
        let store = Zappers::open(dir.path(), "local").unwrap();
        assert_eq!(delivery(&store, "a").reply.as_deref(), Some("report-a"));
        assert!(store.unanswered("release", "other").unwrap());
        let transcript = vec![
            message("a:answer", User),
            message("turn-a", Assistant),
            message("b:answer", User),
            message("turn-b", Assistant),
        ];
        // Rereading A's turn never touches B.
        let again = answered_message(&transcript, "turn-a").unwrap();
        store
            .correlate("release", "other", "report-a", again.as_deref())
            .unwrap();
        assert_eq!(delivery(&store, "b").state, ZapperIssueDeliveryState::Sent);
        let started = answered_message(&transcript, "turn-b").unwrap();
        assert!(
            store
                .correlate("release", "other", "report-b", started.as_deref())
                .unwrap()
        );
        report(&store, "report-b");
        assert_eq!(
            delivery(&store, "b").state,
            ZapperIssueDeliveryState::Answered
        );
        assert_eq!(delivery(&store, "a").reply.as_deref(), Some("report-a"));
        assert_eq!(delivery(&store, "b").reply.as_deref(), Some("report-b"));
    }

    #[test]
    fn a_report_recorded_before_its_link_is_nested_later() {
        let (dir, store) = store();
        board(&store);
        store.command(reply("answer", "report-other")).unwrap();
        sent(&store, "answer:answer", "turn-0");
        // Interrupted between the report and its link (or the transcript
        // hadn't caught up): the next pass links the posted report.
        report(&store, "report-2");
        assert_eq!(entry(&store, "report-2").reply_to, None);
        drop(store);
        let store = Zappers::open(dir.path(), "local").unwrap();
        assert!(
            store
                .correlate("release", "other", "report-2", Some("answer:answer"))
                .unwrap()
        );
        assert_eq!(
            entry(&store, "report-2").reply_to.as_deref(),
            Some("report-other")
        );
    }

    #[test]
    fn a_stop_before_dispatch_cancels_the_answer() {
        let (_dir, store) = store();
        board(&store);
        store.command(reply("answer", "report-other")).unwrap();
        run(
            &store,
            "stop",
            ZapperAction::StopWorker {
                chat_id: "other".into(),
            },
        );
        let cancelled = delivery(&store, "answer");
        assert_eq!(cancelled.state, ZapperIssueDeliveryState::Cancelled);
        let zapper = store.get("release").unwrap();
        let db = store.inner.db.lock().unwrap();
        assert!(
            answer_hold(&db, &zapper, "answer:answer")
                .unwrap()
                .unwrap()
                .contains("You stopped")
        );
    }

    #[test]
    fn an_answer_held_from_a_stopped_thread_goes_back_to_the_zapper() {
        let (_dir, store) = store();
        board(&store);
        ask(&store, Some("worker"));
        run(
            &store,
            "stop",
            ZapperAction::StopWorker {
                chat_id: "worker".into(),
            },
        );
        let revision = store.get("release").unwrap().revision;
        answer(&store, "answer", Some(revision));
        assert!(answers(&store).is_empty());
        assert_eq!(
            delivery(&store, "answer").state,
            ZapperIssueDeliveryState::Held
        );
        // Neither the stopped thread nor the other linked one takes it.
        assert_eq!(
            issue(&store).details.current_owner(),
            Some(ZapperIssueOwner::Zapper)
        );
    }

    #[test]
    fn delivery_states_project_the_same_from_one_batch() {
        let (_dir, store) = store();
        board(&store);
        for n in 0..40 {
            store
                .command(reply(&format!("c{n}"), "report-other"))
                .unwrap();
        }
        sent(&store, "c1:answer", "t");
        sent(&store, "c2:answer", "t");
        store
            .outbox_phase(
                "c3:answer",
                "sending",
                Some("worker conversation unavailable"),
            )
            .unwrap();
        store.outbox_phase("c4:answer", "cancelled", None).unwrap();
        store
            .correlate("release", "other", "report-2", Some("c2:answer"))
            .unwrap();
        report(&store, "report-2");
        let states: Vec<_> = (0..6)
            .map(|n| {
                let d = delivery(&store, &format!("c{n}"));
                (d.state, d.detail)
            })
            .collect();
        use ZapperIssueDeliveryState::*;
        assert_eq!(
            states,
            vec![
                (Queued, None),
                (Sent, None),
                (Answered, None),
                (Retrying, Some("worker conversation unavailable".into())),
                (Cancelled, Some("Stopped before it was sent".into())),
                (Queued, None),
            ]
        );
    }
    fn message(id: &str, role: MessageRole) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: format!("{id}-text"),
                text: id.into(),
            }],
            created_at: 0,
            device_id: "local".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn turns_are_matched_to_the_message_that_started_them() {
        use MessageRole::{Assistant, User};
        let transcript = vec![
            message("kickoff", User),
            message("turn-0", Assistant),
            message("a:answer", User),
            message("turn-1", Assistant),
            message("b:answer", User),
            message("turn-2", Assistant),
            message("typed in chat", User),
            message("turn-3", Assistant),
        ];
        assert_eq!(
            answered_message(&transcript, "turn-1"),
            Some(Some("a:answer".into()))
        );
        assert_eq!(
            answered_message(&transcript, "turn-2"),
            Some(Some("b:answer".into()))
        );
        assert_eq!(answered_message(&transcript, "turn-3"), Some(None));
        assert_eq!(answered_message(&transcript, "turn-0"), Some(None));
        // A transcript that hasn't caught up decides nothing yet.
        assert_eq!(answered_message(&transcript, "turn-9"), None);
    }

    #[test]
    fn updates_record_typed_before_and_after_values() {
        let (_dir, store) = store();
        board(&store);
        let mut details = issue(&store).details;
        details.status = ZapperIssueStatus::InReview;
        details.owner = Some(ZapperIssueOwner::Thread {
            chat_id: "worker".into(),
        });
        run(
            &store,
            "edit",
            ZapperAction::EditIssue {
                issue_id: "fix".into(),
                details,
            },
        );
        let edit = entry(&store, "edit");
        assert!(edit.text.contains("Owner: worker"), "{}", edit.text);
        assert_eq!(
            edit.changes,
            vec![
                change("owner", Some("zapper".into()), Some("worker".into())),
                change("status", Some("backlog".into()), Some("inReview".into())),
            ]
        );
        let mut details = issue(&store).details;
        details.labels = vec!["web".into()];
        details.due_date = Some("2026-12-05".into());
        run(
            &store,
            "details",
            ZapperAction::EditIssue {
                issue_id: "fix".into(),
                details,
            },
        );
        assert_eq!(
            entry(&store, "details").changes,
            vec![
                change("dueDate", None, Some("2026-12-05".into())),
                change("labels", None, Some("web".into())),
            ]
        );
        assert_eq!(
            entry(&store, "link-worker").changes,
            vec![change("thread", None, Some("worker".into()))]
        );
    }
}
