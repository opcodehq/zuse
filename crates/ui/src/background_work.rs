//! What a chat still has running beside its turn: subagents it spawned and
//! background processes (shells, dev servers, monitors). The composer's
//! Background work tray row lists these.

use chrono::{DateTime, TimeZone as _, Utc};
use gpui::SharedString;
use zuse_doc::{MessagePart, SessionMessageEntry, SubagentStatus};
use zuse_protocol::{BackgroundTask, BackgroundTaskKind};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WorkKind {
    /// A subagent: clicking opens its transcript.
    Agent { doc_id: String },
    /// A background process: stoppable when the runtime offers it.
    Task { task_id: String, stoppable: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkItem {
    pub kind: WorkKind,
    pub label: SharedString,
    /// A process's command line, unless it only repeats the label.
    pub command: Option<SharedString>,
    pub started_at: Option<DateTime<Utc>>,
}

impl WorkItem {
    pub(crate) fn key(&self) -> &str {
        match &self.kind {
            WorkKind::Agent { doc_id } => doc_id,
            WorkKind::Task { task_id, .. } => task_id,
        }
    }

    pub(crate) fn is_agent(&self) -> bool {
        matches!(self.kind, WorkKind::Agent { .. })
    }
}

/// Running subagents (in spawn order) followed by running processes (oldest
/// first). Detached agents surface as subagents, so agent-kind tasks are
/// skipped to avoid listing them twice.
pub(crate) fn running_work(
    entries: &[SessionMessageEntry],
    tasks: &[BackgroundTask],
) -> Vec<WorkItem> {
    let mut agents: Vec<(WorkItem, bool)> = Vec::new();
    for entry in entries {
        let spawned_at = Utc.timestamp_millis_opt(entry.created_at).single();
        for part in &entry.parts {
            let MessagePart::Tool {
                call,
                is_error,
                resolved,
                subagent_ref: Some(doc_id),
                subagent_status,
                ..
            } = part
            else {
                continue;
            };
            if !call.is_subagent_spawn() {
                continue;
            }
            let status = subagent_status.unwrap_or(if *is_error {
                SubagentStatus::Failed
            } else if *resolved {
                SubagentStatus::Done
            } else {
                SubagentStatus::Running
            });
            let running = status == SubagentStatus::Running;
            match agents.iter_mut().find(|(item, _)| item.key() == doc_id) {
                Some((item, live)) => {
                    item.label = crate::transcript::subagent_tab_title(call);
                    *live = running;
                }
                None => agents.push((
                    WorkItem {
                        kind: WorkKind::Agent {
                            doc_id: doc_id.clone(),
                        },
                        label: crate::transcript::subagent_tab_title(call),
                        command: None,
                        started_at: spawned_at,
                    },
                    running,
                )),
            }
        }
    }
    let mut processes: Vec<&BackgroundTask> = tasks
        .iter()
        .filter(|task| task.running() && task.kind != BackgroundTaskKind::Agent)
        .collect();
    processes.sort_by_key(|task| task.started_at);
    agents
        .into_iter()
        .filter_map(|(item, running)| running.then_some(item))
        .chain(processes.into_iter().map(|task| {
            let label: SharedString = match task.description.trim() {
                "" => "Background process".into(),
                text => text.to_owned().into(),
            };
            let command = task
                .command
                .as_deref()
                .map(str::trim)
                .filter(|command| !command.is_empty() && *command != label.as_ref())
                .map(|command| SharedString::from(command.to_owned()));
            WorkItem {
                kind: WorkKind::Task {
                    task_id: task.id.clone(),
                    stoppable: task.stoppable,
                },
                label,
                command,
                started_at: Some(task.started_at),
            }
        }))
        .collect()
}

/// The earliest start among the items: the row's overall elapsed time.
pub(crate) fn oldest_start(items: &[WorkItem]) -> Option<DateTime<Utc>> {
    items.iter().filter_map(|item| item.started_at).min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, kind: BackgroundTaskKind, started: i64, ended: bool) -> BackgroundTask {
        BackgroundTask {
            id: id.into(),
            kind,
            description: format!("task {id}"),
            command: Some(format!("task {id}")),
            started_at: Utc.timestamp_opt(started, 0).unwrap(),
            ended_at: ended.then(|| Utc.timestamp_opt(started + 5, 0).unwrap()),
            outcome: None,
            stoppable: true,
        }
    }

    #[test]
    fn lists_running_processes_oldest_first_without_agents_or_finished_ones() {
        let tasks = vec![
            task("late", BackgroundTaskKind::Shell, 300, false),
            task("done", BackgroundTaskKind::Shell, 100, true),
            task("agent", BackgroundTaskKind::Agent, 50, false),
            task("early", BackgroundTaskKind::Shell, 200, false),
        ];
        let work = running_work(&[], &tasks);
        let keys: Vec<_> = work.iter().map(WorkItem::key).collect();
        assert_eq!(keys, ["early", "late"]);
        // A command that repeats the label is not shown twice.
        assert_eq!(work[0].command, None);
        assert_eq!(
            oldest_start(&work),
            Some(Utc.timestamp_opt(200, 0).unwrap())
        );
    }

    #[test]
    fn nothing_running_is_empty() {
        assert!(running_work(&[], &[]).is_empty());
        assert_eq!(oldest_start(&[]), None);
    }
}
