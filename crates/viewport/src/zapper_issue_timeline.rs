//! How an issue's activity reads: discussion as cards, quiet audit events as
//! rows, and longer runs of audit events folded into one summary line.
use zuse_protocol::zappers::*;

/// Runs of at least this many quiet events fold into a summary.
const FOLD: usize = 3;

#[derive(Debug, PartialEq)]
pub enum Item<'a> {
    /// A comment, report or question, with the replies nested under it.
    Card(&'a ZapperIssueActivity),
    /// One quiet event: a status, owner, link or detail change.
    Event(&'a ZapperIssueActivity),
    /// Consecutive quiet events; the first one's id keys its open state.
    Group(Vec<&'a ZapperIssueActivity>),
}

pub fn is_discussion(event: &ZapperIssueActivity) -> bool {
    matches!(event.kind.as_str(), "comment" | "workerReport" | "question")
}

/// A quiet event that still needs to be seen: it never folds.
fn needs_attention(event: &ZapperIssueActivity) -> bool {
    if event.changes.is_empty() {
        return event.text.contains("Needs you") || event.text.contains("Blocked");
    }
    event
        .changes
        .iter()
        .any(|c| c.field == "status" && matches!(c.after.as_deref(), Some("needsYou" | "blocked")))
}

fn flush<'a>(run: &mut Vec<&'a ZapperIssueActivity>, items: &mut Vec<Item<'a>>) {
    if run.len() >= FOLD {
        items.push(Item::Group(std::mem::take(run)));
    } else {
        items.extend(run.drain(..).map(Item::Event));
    }
}

/// Top-level activity in stored order, with quiet runs folded.
pub fn timeline(activity: &[ZapperIssueActivity]) -> Vec<Item<'_>> {
    let mut items = vec![];
    let mut run = vec![];
    for event in activity.iter().filter(|e| e.reply_to.is_none()) {
        if is_discussion(event) {
            flush(&mut run, &mut items);
            items.push(Item::Card(event));
        } else if needs_attention(event) {
            flush(&mut run, &mut items);
            items.push(Item::Event(event));
        } else {
            run.push(event);
        }
    }
    flush(&mut run, &mut items);
    items
}

/// One summary line for a folded run, from what the events record.
pub fn summary(events: &[&ZapperIssueActivity], pull_request: impl Fn(&str) -> String) -> String {
    let mut parts = vec![];
    let updates = events
        .iter()
        .filter(|e| matches!(e.kind.as_str(), "updated" | "created" | "closed"))
        .count();
    if updates > 0 {
        parts.push(format!(
            "{updates} issue {}",
            if updates == 1 { "update" } else { "updates" }
        ));
    }
    let threads = events
        .iter()
        .filter(|e| {
            e.kind == "thread"
                && (e
                    .changes
                    .iter()
                    .any(|c| c.field == "thread" && c.after.is_some())
                    || (e.changes.is_empty() && !e.text.starts_with("Unlinked")))
        })
        .count();
    if threads > 0 {
        parts.push(format!(
            "{threads} {} linked",
            if threads == 1 { "thread" } else { "threads" }
        ));
    }
    for change in events.iter().flat_map(|e| &e.changes) {
        if let ("pullRequest", Some(url)) = (change.field.as_str(), &change.after) {
            parts.push(format!("linked {}", pull_request(url)));
        }
    }
    if let Some(status) = events
        .iter()
        .rev()
        .flat_map(|e| e.changes.iter().rev())
        .find(|c| c.field == "status")
        .and_then(|c| c.after.as_deref())
        .and_then(status_of)
    {
        parts.push(format!("now {}", status.label()));
    }
    if parts.is_empty() {
        parts.push(format!("{} updates", events.len()));
    }
    parts.join(", ")
}

pub fn status_of(wire: &str) -> Option<ZapperIssueStatus> {
    serde_json::from_value(serde_json::Value::String(wire.to_owned())).ok()
}

/// A link inside an event row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Thread(String),
    PullRequest(String),
}

/// What an event row says after its actor. Stored values are plain text,
/// never markup or authority; ids become names only through `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phrase {
    pub action: String,
    pub target: Option<(Target, String)>,
    /// What follows the target, so a long row truncates after the link.
    pub tail: String,
    pub status: Option<ZapperIssueStatus>,
}

/// `None` for entries saved before typed changes: they keep their text.
pub fn phrase(
    event: &ZapperIssueActivity,
    name: impl Fn(&str) -> String,
    pull_request: impl Fn(&str) -> String,
) -> Option<Phrase> {
    if event.kind == "created" {
        return Some(Phrase {
            action: "created the issue".into(),
            target: None,
            tail: String::new(),
            status: None,
        });
    }
    if event.changes.is_empty() {
        return None;
    }
    let mut actions: Vec<String> = vec![];
    let mut target = None;
    let mut status = None;
    // Linking a thread that also takes the issue reads as one act.
    let linked = event
        .changes
        .iter()
        .find(|c| c.field == "thread")
        .and_then(|c| c.after.as_deref());
    for change in &event.changes {
        if change.field == "owner" && change.after.is_some() && change.after.as_deref() == linked {
            continue;
        }
        let (before, after) = (change.before.as_deref(), change.after.as_deref());
        match change.field.as_str() {
            "status" => {
                status = after.and_then(status_of);
                let label = status.map_or("an unknown status", |s| s.label());
                actions.push(format!("moved the issue to {label}"));
            }
            "priority" => actions.push(format!(
                "set priority to {}",
                after
                    .and_then(|p| serde_json::from_value::<ZapperIssuePriority>(p.into()).ok())
                    .map_or("unknown", |p| p.label())
            )),
            "owner" => actions.push(match (before, after) {
                (Some(old), Some(new)) => {
                    format!("assigned {} and unassigned {}", name(new), name(old))
                }
                (None, Some(new)) => format!("assigned {}", name(new)),
                (_, None) => "removed the owner".to_owned(),
            }),
            "thread" | "pullRequest" => {
                let (verb, value) = match (before, after) {
                    (_, Some(v)) => ("linked", v),
                    (Some(v), None) => ("unlinked", v),
                    (None, None) => continue,
                };
                if target.is_none() {
                    actions.insert(0, verb.into());
                    target = Some(if change.field == "thread" {
                        (Target::Thread(value.to_owned()), name(value))
                    } else {
                        (Target::PullRequest(value.to_owned()), pull_request(value))
                    });
                }
            }
            "title" => actions.push(format!(
                "renamed the issue to “{}”",
                after.unwrap_or_default()
            )),
            "dueDate" => actions.push(match after {
                Some(date) => format!("set the due date to {date}"),
                None => "cleared the due date".into(),
            }),
            "labels" => actions.push(match after {
                Some(labels) => format!("set labels to {labels}"),
                None => "cleared the labels".into(),
            }),
            "description" => actions.push("edited the description".into()),
            "doneCriteria" => actions.push("edited what done means".into()),
            "outOfScope" => actions.push("edited what's out of scope".into()),
            _ => actions.push("updated the details".into()),
        }
    }
    actions.dedup();
    let (action, tail) = match (&target, actions.split_first()) {
        (Some(_), Some((verb, rest))) => (
            verb.clone(),
            rest.iter()
                .map(|a| format!("and {a}"))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => (actions.join(" and "), String::new()),
    };
    Some(Phrase {
        action,
        target,
        tail,
        status,
    })
}

/// A thread's confirmed run state, as the host knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// A fresh session report says it is running.
    Running,
    AwaitingInput,
    Failed,
    /// The conversation looks busy, but no fresh session report confirms it.
    Unconfirmed,
}

/// The line above the comment box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Indicator {
    Working(String),
    NeedsInput(String),
    Failed(String),
    Unconfirmed(String),
    Queued(String, Option<u32>),
    ReplyQueued(String),
    ReplyRetrying(String),
}
impl Indicator {
    pub fn text(&self) -> String {
        match self {
            Self::Working(n) => format!("{n} is working"),
            Self::NeedsInput(n) => format!("{n} needs input"),
            Self::Failed(n) => format!("{n} stopped with an error"),
            Self::Unconfirmed(n) => format!("Can't confirm {n} is running"),
            Self::Queued(n, Some(p)) => format!("{n} is queued (#{p})"),
            Self::Queued(n, None) => format!("{n} is queued"),
            Self::ReplyQueued(n) => format!("Your reply is queued for {n}"),
            Self::ReplyRetrying(n) => format!("Retrying delivery to {n}"),
        }
    }
    pub fn live(&self) -> bool {
        matches!(self, Self::Working(_))
    }
}

/// What the issue's accountable thread (else the Zapper) is doing now,
/// from confirmed run state only. An undelivered reply comes first.
pub fn indicator(
    zapper: &Zapper,
    issue: &ZapperIssue,
    run: impl Fn(&str) -> Option<RunState>,
    name: impl Fn(&str) -> String,
) -> Option<Indicator> {
    if issue.details.status.terminal() {
        return None;
    }
    if let Some(delivery) = issue
        .activity
        .iter()
        .rev()
        .filter_map(|a| a.delivery.as_ref())
        .find(|d| d.chat_id != zapper.coordinator_chat_id)
    {
        match delivery.state {
            ZapperIssueDeliveryState::Queued => {
                return Some(Indicator::ReplyQueued(name(&delivery.chat_id)));
            }
            ZapperIssueDeliveryState::Retrying => {
                return Some(Indicator::ReplyRetrying(name(&delivery.chat_id)));
            }
            _ => {}
        }
    }
    let chat = match issue.details.current_owner() {
        Some(ZapperIssueOwner::Thread { chat_id }) => chat_id,
        Some(ZapperIssueOwner::User) => return None,
        _ => zapper.coordinator_chat_id.clone(),
    };
    if let Some(worker) = zapper
        .workers
        .iter()
        .find(|w| w.chat_id == chat && w.queued())
    {
        return Some(Indicator::Queued(name(&chat), worker.queue_position));
    }
    Some(match run(&chat)? {
        RunState::Running => Indicator::Working(name(&chat)),
        RunState::AwaitingInput => Indicator::NeedsInput(name(&chat)),
        RunState::Failed => Indicator::Failed(name(&chat)),
        RunState::Unconfirmed => Indicator::Unconfirmed(name(&chat)),
    })
}

/// A routed comment's footnote, and whether it needs attention.
pub fn delivery_note(delivery: &ZapperIssueDelivery, name: &str) -> (String, bool) {
    use ZapperIssueDeliveryState::*;
    let with = |text: String| match delivery.detail.as_deref() {
        Some(detail) => format!("{text} · {detail}"),
        None => text,
    };
    match delivery.state {
        Queued => (with(format!("Queued for {name}")), false),
        Retrying => (with(format!("Couldn't reach {name} yet")), true),
        Sent => (with(format!("Sent to {name}")), false),
        Answered => (format!("Answered by {name}"), false),
        Cancelled | Held => (with(format!("Not sent to {name}")), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(id: &str, kind: &str, changes: Vec<ZapperIssueChange>) -> ZapperIssueActivity {
        ZapperIssueActivity {
            id: id.into(),
            kind: kind.into(),
            text: id.into(),
            changes,
            ..Default::default()
        }
    }
    fn change(field: &str, before: Option<&str>, after: Option<&str>) -> ZapperIssueChange {
        ZapperIssueChange {
            field: field.into(),
            before: before.map(Into::into),
            after: after.map(Into::into),
        }
    }
    fn status(id: &str, to: &str) -> ZapperIssueActivity {
        event(
            id,
            "updated",
            vec![change("status", Some("todo"), Some(to))],
        )
    }
    #[test]
    fn quiet_runs_fold_but_discussion_and_attention_do_not() {
        let activity = vec![
            event("created", "created", vec![]),
            status("s1", "inProgress"),
            event("t1", "thread", vec![change("thread", None, Some("w"))]),
            event("c1", "comment", vec![]),
            status("s2", "inReview"),
            status("blocked", "blocked"),
            status("s3", "inProgress"),
        ];
        let ids: Vec<String> = timeline(&activity)
            .iter()
            .map(|i| match i {
                Item::Card(e) => format!("card:{}", e.id),
                Item::Event(e) => format!("event:{}", e.id),
                Item::Group(g) => format!("group:{}", g.len()),
            })
            .collect();
        assert_eq!(
            ids,
            [
                "group:3",
                "card:c1",
                "event:s2",
                "event:blocked",
                "event:s3"
            ]
        );
    }
    #[test]
    fn replies_never_appear_at_the_top_level() {
        let mut reply = event("r", "workerReport", vec![]);
        reply.reply_to = Some("c".into());
        let activity = vec![event("c", "comment", vec![]), reply];
        assert_eq!(timeline(&activity).len(), 1);
    }
    #[test]
    fn summaries_count_only_what_was_recorded() {
        let created = event("created", "created", vec![]);
        let moved = status("s", "inProgress");
        let linked = event(
            "pr",
            "pullRequest",
            vec![change(
                "pullRequest",
                None,
                Some("https://github.com/a/zuse/pull/12"),
            )],
        );
        let text = summary(&[&created, &moved, &linked], |_| "zuse#12".into());
        assert_eq!(text, "2 issue updates, linked zuse#12, now In progress");
    }
    #[test]
    fn phrases_name_owners_and_link_targets() {
        let name = |id: &str| match id {
            "user" => "Swaraj".to_owned(),
            "worker-1" => "Deploy thread".to_owned(),
            other => other.to_owned(),
        };
        let owner = event(
            "e",
            "updated",
            vec![change("owner", Some("user"), Some("worker-1"))],
        );
        let p = phrase(&owner, name, |u| u.to_owned()).unwrap();
        assert_eq!(p.action, "assigned Deploy thread and unassigned Swaraj");
        let delegate = event(
            "g",
            "thread",
            vec![
                change("thread", None, Some("worker-1")),
                change("owner", Some("zapper"), Some("worker-1")),
                change("status", Some("todo"), Some("inProgress")),
            ],
        );
        let p = phrase(&delegate, name, |u| u.to_owned()).unwrap();
        assert_eq!(
            (p.action.as_str(), p.tail.as_str()),
            ("linked", "and moved the issue to In progress")
        );
        let link = event(
            "l",
            "thread",
            vec![change("thread", None, Some("worker-1"))],
        );
        let p = phrase(&link, name, |u| u.to_owned()).unwrap();
        assert_eq!(p.action, "linked");
        assert_eq!(
            p.target,
            Some((Target::Thread("worker-1".into()), "Deploy thread".into()))
        );
        let due = event(
            "d",
            "updated",
            vec![change("dueDate", None, Some("2026-12-05"))],
        );
        assert_eq!(
            phrase(&due, name, |u| u.to_owned()).unwrap().action,
            "set the due date to 2026-12-05"
        );
        // Entries from before typed changes keep their own text.
        assert_eq!(
            phrase(&event("old", "updated", vec![]), name, |u| u.to_owned()),
            None
        );
        // An unknown stored value reads as unknown, never as markup or a guess.
        let odd = event(
            "o",
            "updated",
            vec![change("status", None, Some("<b>x</b>"))],
        );
        assert_eq!(
            phrase(&odd, name, |u| u.to_owned()).unwrap().action,
            "moved the issue to an unknown status"
        );
    }
    fn fixture(
        owner: Option<ZapperIssueOwner>,
        delivery: Option<ZapperIssueDeliveryState>,
    ) -> (Zapper, ZapperIssue) {
        let mut comment = event("c", "comment", vec![]);
        comment.delivery = delivery.map(|state| ZapperIssueDelivery {
            chat_id: "w".into(),
            state,
            detail: None,
            reply: None,
        });
        let issue = ZapperIssue {
            id: "i".into(),
            number: 1,
            details: ZapperIssueDetails {
                title: "T".into(),
                status: ZapperIssueStatus::InProgress,
                owner,
                ..Default::default()
            },
            created_at: 0,
            updated_at: 0,
            threads: vec!["w".into()],
            pull_requests: vec![],
            activity: vec![comment],
        };
        let zapper: Zapper = serde_json::from_value(serde_json::json!({
            "id": "z", "revision": 1,
            "config": {"name": "Release", "purpose": "", "homeDeviceId": "d", "repositories": [],
                "coordinatorModel": {"harness": "codex", "model": null}, "workerDeviceId": "d",
                "maxConcurrentWorkers": 1, "maxFollowups": 1},
            "status": "active", "coordinatorChatId": "coord", "previousCoordinatorChats": [],
            "workers": [{"chatId": "w", "deviceId": "d", "spaceId": "s", "objective": "o", "followups": 0}],
            "decisions": []
        }))
        .unwrap();
        (zapper, issue)
    }
    #[test]
    fn the_indicator_shows_only_confirmed_execution() {
        let thread = Some(ZapperIssueOwner::Thread {
            chat_id: "w".into(),
        });
        let name = |id: &str| id.to_uppercase();
        let (z, i) = fixture(thread.clone(), None);
        assert_eq!(
            indicator(&z, &i, |_| Some(RunState::Running), name),
            Some(Indicator::Working("W".into()))
        );
        assert_eq!(indicator(&z, &i, |_| None, name), None);
        assert_eq!(
            indicator(&z, &i, |_| Some(RunState::Unconfirmed), name)
                .unwrap()
                .text(),
            "Can't confirm W is running"
        );
        let (z, i) = fixture(thread, Some(ZapperIssueDeliveryState::Retrying));
        assert_eq!(
            indicator(&z, &i, |_| Some(RunState::Running), name),
            Some(Indicator::ReplyRetrying("W".into()))
        );
        let (z, i) = fixture(Some(ZapperIssueOwner::Zapper), None);
        assert_eq!(
            indicator(
                &z,
                &i,
                |c| (c == "coord").then_some(RunState::Running),
                name
            ),
            Some(Indicator::Working("COORD".into()))
        );
    }
}
