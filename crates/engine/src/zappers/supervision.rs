//! Model-free reconciliation and durable delivery. Unknown execution keeps its slot.
use super::*;
use crate::{DocHost, SessionsEngine, WorkspaceHost};
use sha2::{Digest, Sha256};
use zuse_doc::SessionCommandPayload;
use zuse_protocol::{ChatConfig, SandboxLevel, SessionStatus, WorktreeSpec};

pub struct ZapperSupervisor(tokio::task::JoinHandle<()>);
impl Drop for ZapperSupervisor {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl ZapperSupervisor {
    pub fn stop(&self) {
        self.0.abort();
    }
    pub fn start(
        zappers: Zappers,
        sessions: SessionsEngine,
        workspace: WorkspaceHost,
        docs: DocHost,
    ) -> Option<Self> {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        Some(Self(runtime.spawn(async move {
            let mut changed = zappers.watch();
            let mut statuses = sessions.watch_sessions();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3));
            loop {
                tokio::select! { _ = tick.tick() => {}, value = changed.changed() => { if value.is_err() { break; } }, value = statuses.changed() => { if value.is_err() { break; } } }
                let bindings=match zappers.sync_worker_bindings().await {Ok(bindings)=>bindings,Err(error)=>{tracing::warn!(%error,"worker binding reconciliation failed");continue;}};
                if let Err(error) = reconcile(&zappers, &sessions, &workspace, &docs, &bindings) {
                    tracing::warn!(%error, "Zapper supervision needs reconciliation");
                }
            }
        })))
    }
}

/// Who ended a turn that stopped without completing. A message the user
/// sent to the thread after the Zapper's (and after any Stop of theirs)
/// redirected the turn: nothing needs continuing, so no hold (`None`).
/// Otherwise a Stop the user sent after the Zapper's message holds it for the
/// user, and with no Stop at all the thread ended by itself (its runtime
/// ended, or the engine stopped it for memory). The Zapper's own commands
/// carry its outbox ids.
pub(super) fn abort_hold(
    commands: &[zuse_doc::SessionCommandEntry],
    sent_at: i64,
    zapper_commands: &[String],
) -> Option<&'static str> {
    let users = commands
        .iter()
        .filter(|c| c.issued_at >= sent_at && !zapper_commands.contains(&c.id));
    let last_stop = users
        .clone()
        .filter(|c| matches!(c.payload, SessionCommandPayload::Interrupt {}))
        .map(|c| c.issued_at)
        .max();
    let redirected = users.clone().any(|c| {
        matches!(
            c.payload,
            SessionCommandPayload::Run { .. }
                | SessionCommandPayload::Steer { .. }
                | SessionCommandPayload::RespondInput { .. }
                | SessionCommandPayload::ChatCommand { .. }
        ) && last_stop.is_none_or(|stop| c.issued_at >= stop)
    });
    if redirected {
        None
    } else if last_stop.is_some() {
        Some(HOLD_USER)
    } else {
        Some(HOLD_THREAD_STOPPED)
    }
}

fn config(model: &ZapperModel, sandbox: Option<SandboxLevel>) -> Result<ChatConfig> {
    Ok(ChatConfig {
        harness: serde_json::from_value(serde_json::json!(model.harness))?,
        model: model.model.clone(),
        reasoning: None,
        model_options: Default::default(),
        plan: false,
        sandbox: sandbox.unwrap_or(SandboxLevel::Auto),
    })
}
pub(super) fn event_id(parts: &[&str]) -> String {
    format!("{:x}", Sha256::digest(parts.join("\0")))
}

fn bounded_report(text: &str) -> String {
    let mut chars = text.chars();
    let mut report: String = chars.by_ref().take(16384).collect();
    if chars.next().is_some() {
        report.push('…');
    }
    report
}

/// Whether a pending delivery to `chat` may go out now. A Zapper runs at
/// most `maxConcurrentWorkers` threads; a new thread that finds every slot
/// taken is marked queued and stays pending, and a later pass starts it once
/// a running thread settles. Queued threads start in the order they were
/// delegated because the outbox is walked in that order.
fn admit(
    zappers: &Zappers,
    zapper: &Zapper,
    id: &str,
    chat: &str,
    action: &ZapperAction,
    busy: impl Fn(&str) -> bool,
) -> Result<bool> {
    let mut reserved = zappers.reserved_workers(&zapper.id)?;
    reserved.extend(
        zapper
            .workers
            .iter()
            .filter(|worker| busy(&worker.chat_id))
            .map(|worker| worker.chat_id.clone()),
    );
    if reserved.contains(chat) {
        return Ok(false);
    }
    if reserved.len() >= zapper.config.max_concurrent_workers as usize {
        if matches!(action, ZapperAction::Delegate { .. }) {
            zappers.mark_queued(id)?;
        }
        return Ok(false);
    }
    Ok(!busy(chat))
}

fn archive_unstarted_worker(workspace: &WorkspaceHost, zapper: &Zapper, chat: &str) -> Result<()> {
    let worker = zapper
        .workers
        .iter()
        .find(|worker| worker.chat_id == chat)
        .context("worker not found")?;
    workspace.create_chat_with_parent(
        chat,
        Some(&worker.space_id),
        Some(&worker.device_id),
        None,
        None,
        None,
    )?;
    ensure!(
        workspace.set_chat_archived(chat, true)?,
        "worker conversation unavailable"
    );
    Ok(())
}

fn reconcile(
    zappers: &Zappers,
    sessions: &SessionsEngine,
    workspace: &WorkspaceHost,
    docs: &DocHost,
    bindings: &std::collections::HashSet<(String, String)>,
) -> Result<()> {
    let _admission = zappers
        .inner
        .admission
        .lock()
        .map_err(|_| anyhow::anyhow!("Zapper admission unavailable"))?;
    let all = zappers.list()?;
    let mut statuses = workspace.read_sessions()?;
    let local = sessions.watch_sessions().borrow().clone();
    statuses.retain(|s| s.device_id != zappers.inner.device);
    statuses.extend(local);
    for zapper in &all {
        // Repair a crash between durable creation and conversation materialization.
        workspace.create_chat_with_parent(
            &zapper.coordinator_chat_id,
            None,
            Some(&zapper.config.home_device_id),
            Some(config(&zapper.config.coordinator_model, None)?),
            Some(
                zappers
                    .workspace(&zapper.id)?
                    .to_string_lossy()
                    .into_owned(),
            ),
            None,
        )?;
        let mut selected = workspace
            .chat_config(&zapper.coordinator_chat_id)
            .unwrap_or(config(&zapper.config.coordinator_model, None)?);
        // Coordinators delegate from their own folder, so a stored plan flag
        // (older Zappers were forced into it) would leave them unable to act.
        if selected.plan {
            selected.plan = false;
            selected.sandbox = SandboxLevel::Auto;
        }
        if workspace.chat_config(&zapper.coordinator_chat_id).as_ref() != Some(&selected) {
            workspace.set_chat_config(&zapper.coordinator_chat_id, &selected)?;
        }
        if matches!(
            zapper.status,
            ZapperStatus::Stopped | ZapperStatus::Archived
        ) {
            let id = event_id(&[&zapper.id, &zapper.revision.to_string(), "stop-coordinator"]);
            docs.queue_identified_command(
                &zapper.coordinator_chat_id,
                SessionCommandPayload::Interrupt {},
                vec![],
                id,
            )?;
        }
        for worker in &zapper.workers {
            let Some(status) = statuses
                .iter()
                .find(|s| s.chat_id == worker.chat_id && s.device_id == worker.device_id)
            else {
                continue;
            };
            zappers.settle_hold(
                &zapper.id,
                &worker.chat_id,
                status.status == SessionStatus::Working,
                status.last_completed_turn.as_deref(),
            )?;
            if status.status == SessionStatus::Working {
                continue;
            }
            let mut db = zappers
                .inner
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?;
            let tx = db.transaction()?;
            let mut stmt = tx.prepare("SELECT o.id,d.baseline,d.sent_at FROM zapper_outbox o JOIN zapper_delivery d ON d.id=o.id WHERE o.zapper=?1 AND o.chat=?2 AND o.phase='sent'")?;
            let deliveries = stmt
                .query_map(params![zapper.id, worker.chat_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            let zapper_commands: Vec<String> = tx
                .prepare("SELECT id FROM zapper_outbox WHERE zapper=?1 AND chat=?2")?
                .query_map(params![zapper.id, worker.chat_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut interrupted = false;
            for (id, baseline, sent_at) in deliveries {
                let fresh = status.updated_at.timestamp_millis() > sent_at
                    || status.last_completed_turn != baseline;
                let aborted = status.status == SessionStatus::Idle
                    && status.updated_at.timestamp_millis() > sent_at
                    && status.last_completed_turn == baseline;
                if fresh
                    && (status.status == SessionStatus::Errored
                        || (status.status == SessionStatus::Idle
                            && status.last_completed_turn != baseline)
                        || aborted)
                {
                    tx.execute("UPDATE zapper_outbox SET phase='settled' WHERE id=?1", [id])?;
                    if aborted || status.status == SessionStatus::Errored {
                        let reason = if status.status == SessionStatus::Errored {
                            Some(HOLD_THREAD_FAILED)
                        } else {
                            let commands = docs
                                .open(&worker.chat_id)
                                .ok()
                                .and_then(|doc| doc.doc_arc().read_commands().ok())
                                .unwrap_or_default();
                            abort_hold(&commands, sent_at, &zapper_commands)
                        };
                        // None: the user redirected the turn with a message
                        // of their own, so nothing waits on anyone.
                        if let Some(reason) = reason {
                            // A hold written when the stop was issued already
                            // says who stopped the thread; either way the
                            // thread is seen stopped at this turn.
                            tx.execute(
                                "INSERT OR IGNORE INTO zapper_worker_holds(zapper,chat,reason) VALUES(?1,?2,?3)",
                                params![zapper.id, worker.chat_id, reason],
                            )?;
                            tx.execute(
                                "UPDATE zapper_worker_holds SET stamped=1, turn=?3 WHERE zapper=?1 AND chat=?2 AND stamped=0",
                                params![zapper.id, worker.chat_id, status.last_completed_turn],
                            )?;
                            interrupted |= aborted;
                        }
                    }
                }
            }
            let hold: Option<String> = tx
                .query_row(
                    "SELECT reason FROM zapper_worker_holds WHERE zapper=?1 AND chat=?2",
                    params![zapper.id, worker.chat_id],
                    |r| r.get(0),
                )
                .optional()?;
            tx.commit()?;
            drop(db);
            let kind = match status.status {
                SessionStatus::Idle if interrupted => "interrupted",
                SessionStatus::Idle if status.last_completed_turn.is_some() => "settled",
                SessionStatus::AwaitingInput => "needsInput",
                SessionStatus::Errored => "failed",
                _ => continue,
            };
            let marker = if interrupted {
                status.updated_at.to_rfc3339()
            } else {
                status
                    .last_completed_turn
                    .clone()
                    .unwrap_or_else(|| status.updated_at.to_rfc3339())
            };
            let report_id = event_id(&[&zapper.id, &worker.chat_id, kind, &marker]);
            // A settled turn answers a person's routed comment only when that
            // comment's message started it. Checked before the report is
            // recorded, and again later if the transcript hadn't caught up.
            if let (false, Some(turn)) = (interrupted, status.last_completed_turn.as_deref())
                && kind == "settled"
                && !issues::checked_turn(&report_id)
                && zappers.unanswered(&zapper.id, &worker.chat_id)?
                && let Some(started) = docs
                    .open(&worker.chat_id)
                    .ok()
                    .and_then(|doc| doc.doc_arc().read_entries().ok())
                    .and_then(|entries| issues::answered_message(&entries, turn))
            {
                // The message that started this exact turn decides, whichever
                // routed comments are still queued behind it.
                zappers.correlate(&zapper.id, &worker.chat_id, &report_id, started.as_deref())?;
                // A transcript that has the turn can't change that answer.
                issues::remember_turn(&report_id);
            }
            if zappers
                .events(&zapper.id)?
                .iter()
                .any(|event| event.id == report_id)
            {
                continue;
            }
            let summary = if interrupted {
                match hold.as_deref().map(HoldSource::of) {
                    Some(HoldSource::Zapper) => {
                        "Worker was stopped by the Zapper. You can resume it with messageWorker."
                            .into()
                    }
                    Some(HoldSource::Thread) => {
                        "Worker stopped before finishing its turn, without anyone stopping it. You can resume it with messageWorker."
                            .into()
                    }
                    _ => "Worker was stopped by the user. Explicit user continuation is required."
                        .into(),
                }
            } else {
                let full = docs
                    .open(&worker.chat_id)
                    .ok()
                    .and_then(|doc| doc.doc_arc().read_entries().ok())
                    .and_then(|entries| {
                        entries
                            .into_iter()
                            .rev()
                            .filter(|entry| entry.role == zuse_model::MessageRole::Assistant)
                            .find_map(|entry| {
                                let text = entry
                                    .parts
                                    .into_iter()
                                    .filter_map(|part| match part {
                                        zuse_model::MessagePart::Text { text, .. } => Some(text),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                (!text.trim().is_empty()).then_some(text)
                            })
                    });
                full.map(|text| bounded_report(&text))
                    .or_else(|| {
                        workspace
                            .chat(&worker.chat_id)
                            .ok()
                            .flatten()
                            .and_then(|c| c.last_message_preview)
                            .map(|text| format!("{text}…"))
                    })
                    .unwrap_or_else(|| format!("Worker update: {kind}"))
            };
            zappers.record_event(&ZapperEvent {
                id: report_id,
                zapper_id: zapper.id.clone(),
                worker_chat_id: worker.chat_id.clone(),
                kind: kind.into(),
                summary,
                acknowledged: false,
            })?;
        }
        if zapper.status == ZapperStatus::Active
            && !sessions.turn_in_flight(&zapper.coordinator_chat_id)
        {
            // Everything that arrived since the last wake goes out as one
            // message, so five finished threads cost one coordinator turn.
            let mut pending = vec![];
            for event in zappers
                .events(&zapper.id)?
                .into_iter()
                .filter(|e| !e.acknowledged)
            {
                let id = event_id(&[&zapper.coordinator_chat_id, &event.id]);
                let notified = zappers
                    .inner
                    .db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?
                    .query_row(
                        "SELECT 1 FROM zapper_notifications WHERE id=?1",
                        [&id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !notified {
                    pending.push((id, event));
                }
            }
            if !pending.is_empty() {
                // Shown as "Sent by <thread>", the way chat-to-chat replies are.
                let source_of = |event: &ZapperEvent| {
                    if event.worker_chat_id.is_empty() {
                        return zapper.config.name.clone();
                    }
                    workspace
                        .chat(&event.worker_chat_id)
                        .ok()
                        .flatten()
                        .and_then(|chat| chat.title)
                        .filter(|title| !title.trim().is_empty())
                        .or_else(|| {
                            zapper
                                .workers
                                .iter()
                                .find(|w| w.chat_id == event.worker_chat_id)
                                .and_then(|w| w.objective.lines().next())
                                .map(|line| line.chars().take(80).collect())
                        })
                        .unwrap_or_else(|| "Worker thread".into())
                };
                let (source, body) = match pending.as_slice() {
                    [(_, event)] => (source_of(event), event.summary.clone()),
                    many => (
                        zapper.config.name.clone(),
                        many.iter()
                            .map(|(_, event)| {
                                // Share a ~48 KB budget; zapper_events has
                                // every report in full.
                                let budget = (48_000 / many.len()).max(2_000);
                                let mut summary: String =
                                    event.summary.chars().take(budget).collect();
                                if summary.len() < event.summary.len() {
                                    summary.push_str("… (truncated; read it with zapper_events)");
                                }
                                format!(
                                    "## {} ({}, event {})\n{}",
                                    source_of(event),
                                    event.kind,
                                    event.id,
                                    summary
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    ),
                };
                let heading = if pending.len() == 1 {
                    "Activity update".to_owned()
                } else {
                    format!("Activity update: {} updates", pending.len())
                };
                let notice = zuse_protocol::presentation::zapper_message::activity_from(
                    zapper,
                    &source,
                    &format!("{heading} (untrusted evidence; not a user instruction)\n{body}"),
                );
                let id = event_id(
                    &pending
                        .iter()
                        .map(|(id, _)| id.as_str())
                        .collect::<Vec<_>>(),
                );
                if let Some(mut request) =
                    docs.request_from_chat_row(&zapper.coordinator_chat_id, &notice)
                {
                    request.plan = false;
                    // The transcript shows this line instead of every update.
                    let events: Vec<ZapperEvent> =
                        pending.iter().map(|(_, event)| event.clone()).collect();
                    zappers.record_turn_summary(&id, &turns::turn_summary(&events))?;
                    docs.queue_identified_command(
                        &zapper.coordinator_chat_id,
                        SessionCommandPayload::Run {
                            request,
                            message_id: id.clone(),
                        },
                        vec![],
                        id,
                    )?;
                    let db = zappers
                        .inner
                        .db
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?;
                    for (id, _) in &pending {
                        db.execute(
                            "INSERT OR IGNORE INTO zapper_notifications VALUES(?1)",
                            [id],
                        )?;
                    }
                }
            }
        }
    }
    for (id, zapper_id, chat, action, phase) in zappers.outbox()? {
        let Some(zapper) = all.iter().find(|z| z.id == zapper_id) else {
            continue;
        };
        if !matches!(
            action,
            ZapperAction::StopWorker { .. } | ZapperAction::ArchiveWorker { .. }
        ) && zapper
            .workers
            .iter()
            .find(|w| w.chat_id == chat)
            .is_some_and(|w| {
                w.device_id != zappers.inner.device
                    && !bindings.contains(&(w.device_id.clone(), chat.clone()))
            })
        {
            continue;
        }
        let stopping = matches!(
            action,
            ZapperAction::StopWorker { .. } | ZapperAction::ArchiveWorker { .. }
        );
        if !stopping && zapper.status != ZapperStatus::Active {
            continue;
        }
        // The dispatch point for a routed comment. This whole pass holds the
        // admission lock that every command takes, so a stop, release or
        // close commits either before this check (and cancels it here) or
        // after the message is queued (and then interrupts the running turn
        // like any other stop); a queued message can't be recalled.
        if !stopping {
            let hold = issues::answer_hold(
                &zappers
                    .inner
                    .db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?
                    as &Connection,
                zapper,
                &id,
            )?;
            if let Some(reason) = hold {
                zappers.outbox_phase(&id, "cancelled", Some(&reason))?;
                continue;
            }
        }
        if phase == "pending" && !stopping {
            let busy = |chat: &str| {
                statuses.iter().any(|s| {
                    s.chat_id == chat
                        && matches!(
                            s.status,
                            SessionStatus::Working | SessionStatus::AwaitingInput
                        )
                })
            };
            if !admit(zappers, zapper, &id, &chat, &action, busy)? {
                continue;
            }
        }
        let baseline = statuses
            .iter()
            .find(|s| s.chat_id == chat)
            .and_then(|s| s.last_completed_turn.clone());
        if phase == "pending" {
            let mut db = zappers
                .inner
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?;
            let tx = db.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO zapper_delivery VALUES(?1,?2,?3)",
                params![
                    id,
                    baseline,
                    statuses
                        .iter()
                        .find(|s| s.chat_id == chat)
                        .map(|s| s.updated_at.timestamp_millis())
                        .unwrap_or(0)
                ],
            )?;
            tx.execute(
                "UPDATE zapper_outbox SET phase='sending' WHERE id=?1 AND phase='pending'",
                [&id],
            )?;
            tx.execute("DELETE FROM zapper_queue WHERE id=?1", [&id])?;
            tx.commit()?;
        }
        let delivery: Result<()> = (|| {
            if stopping && workspace.chat(&chat)?.is_none() {
                // A cancelled queued delegate never created a conversation.
                // Stop has nothing to interrupt; archive still needs a durable
                // chat row so it stays out of the sidebar after reconnecting.
                if matches!(action, ZapperAction::ArchiveWorker { .. }) {
                    archive_unstarted_worker(workspace, zapper, &chat)?;
                }
                return Ok(());
            }
            let payload = match &action {
                ZapperAction::Delegate {
                    worker,
                    model,
                    base,
                    ..
                } => {
                    let space = workspace
                        .space(&worker.space_id)?
                        .context("worker repository unavailable")?;
                    ensure!(
                        space.device_id == worker.device_id,
                        "worker repository moved"
                    );
                    workspace.create_chat_with_parent(
                        &chat,
                        Some(&worker.space_id),
                        Some(&worker.device_id),
                        Some(config(model, Some(SandboxLevel::Auto))?),
                        None,
                        None,
                    )?;
                    let mut request = docs
                        .request_from_chat_row(
                            &chat,
                            &zuse_protocol::presentation::zapper_message::envelope(
                                zapper,
                                &worker.objective,
                            ),
                        )
                        .context("worker conversation unavailable")?;
                    request.worktree = Some(WorktreeSpec {
                        repo_path: space.path,
                        base: base.clone(),
                        space_id: Some(space.id),
                    });
                    SessionCommandPayload::Run {
                        request,
                        message_id: id.clone(),
                    }
                }
                ZapperAction::MessageWorker { text, .. } => SessionCommandPayload::Run {
                    request: docs
                        .request_from_chat_row(
                            &chat,
                            &zuse_protocol::presentation::zapper_message::envelope(zapper, text),
                        )
                        .context("worker conversation unavailable")?,
                    message_id: id.clone(),
                },
                ZapperAction::StopWorker { .. } | ZapperAction::ArchiveWorker { .. } => {
                    SessionCommandPayload::Interrupt {}
                }
                _ => bail!("unsupported Zapper delivery"),
            };
            docs.queue_identified_command(&chat, payload, vec![], id.clone())?;
            if matches!(action, ZapperAction::ArchiveWorker { .. }) {
                workspace.set_chat_archived(&chat, true)?;
            }
            Ok(())
        })();
        match delivery {
            Ok(()) => zappers.outbox_phase(&id, if stopping { "settled" } else { "sent" }, None)?,
            Err(error) => {
                zappers.outbox_phase(&id, "sending", Some(&error.to_string()))?;
                zappers.record_event(&ZapperEvent {
                    id: event_id(&[&id, "delivery"]),
                    zapper_id: zapper.id.clone(),
                    worker_chat_id: chat,
                    kind: "deliveryPending".into(),
                    summary: error.to_string(),
                    acknowledged: false,
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod abort_tests {
    use super::*;
    use zuse_doc::{SessionCommandEntry, SessionCommandStatus};

    fn stop(id: &str, at: i64) -> SessionCommandEntry {
        SessionCommandEntry {
            id: id.into(),
            payload: SessionCommandPayload::Interrupt {},
            issued_by: "someone".into(),
            issued_at: at,
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Applied,
            resolution: None,
        }
    }

    fn message(id: &str, at: i64) -> SessionCommandEntry {
        SessionCommandEntry {
            payload: SessionCommandPayload::Steer {
                prompt: "do this instead".into(),
                message_id: None,
            },
            ..stop(id, at)
        }
    }

    #[test]
    fn aborts_are_held_for_whoever_caused_them() {
        let zapper = vec!["zapper-stop".to_owned()];
        // Nobody stopped it: the thread ended by itself.
        assert_eq!(abort_hold(&[], 100, &zapper), Some(HOLD_THREAD_STOPPED));
        // A Stop from before the Zapper's message doesn't count.
        assert_eq!(
            abort_hold(&[stop("old", 50)], 100, &zapper),
            Some(HOLD_THREAD_STOPPED)
        );
        // The Zapper's own stop is its outbox id.
        assert_eq!(
            abort_hold(&[stop("zapper-stop", 150)], 100, &zapper),
            Some(HOLD_THREAD_STOPPED)
        );
        // Anyone else's Stop after the message is the user's.
        assert_eq!(
            abort_hold(&[stop("button", 150)], 100, &zapper),
            Some(HOLD_USER)
        );
    }

    #[test]
    fn a_turn_the_user_redirected_needs_no_continuation() {
        let zapper = vec!["zapper-stop".to_owned()];
        // Stop then a new message: the user redirected the turn.
        assert_eq!(
            abort_hold(&[stop("button", 150), message("new", 151)], 100, &zapper),
            None
        );
        // A message that replaced the turn without a separate Stop.
        assert_eq!(abort_hold(&[message("new", 150)], 100, &zapper), None);
        // A message before a later Stop doesn't undo that Stop.
        assert_eq!(
            abort_hold(&[message("new", 150), stop("button", 160)], 100, &zapper),
            Some(HOLD_USER)
        );
        // The Zapper's own messages are not the user's.
        assert_eq!(
            abort_hold(&[message("zapper-stop", 150)], 100, &zapper),
            Some(HOLD_THREAD_STOPPED)
        );
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    use crate::zappers::tests::{command, config, worker};

    fn delegate(store: &Zappers, chat: &str, issue: &str) {
        store
            .command(command(
                "release",
                &format!("issue-{issue}"),
                ZapperAction::CreateIssue {
                    issue_id: issue.into(),
                    details: ZapperIssueDetails {
                        title: format!("Fix {issue}"),
                        description: "Pass acceptance".into(),
                        assigned: true,
                        ..Default::default()
                    },
                },
            ))
            .unwrap();
        store
            .command(command(
                "release",
                chat,
                ZapperAction::Delegate {
                    issue_id: Some(issue.into()),
                    worker: ZapperWorker {
                        chat_id: chat.into(),
                        objective: format!("Fix {issue}"),
                        // A client can't queue a thread by claiming a position.
                        queue_position: Some(7),
                        ..worker()
                    },
                    model: config().coordinator_model,
                    base: "origin/main".into(),
                },
            ))
            .unwrap();
    }

    /// One supervisor pass over pending deliveries, standing in for the
    /// delivery itself: what's admitted is marked sent. No thread is busy
    /// outside the Zapper's own deliveries.
    fn pass(store: &Zappers) -> Vec<String> {
        let mut started = vec![];
        for (id, zapper_id, chat, action, phase) in store.outbox().unwrap() {
            // Stops bypass admission, as in `reconcile`.
            if phase != "pending" || !matches!(action, ZapperAction::Delegate { .. }) {
                continue;
            }
            let zapper = store.get(&zapper_id).unwrap();
            if admit(store, &zapper, &id, &chat, &action, |_| false).unwrap() {
                store.outbox_phase(&id, "sent", None).unwrap();
                started.push(chat);
            }
        }
        started
    }

    fn positions(store: &Zappers) -> Vec<(String, Option<u32>)> {
        store
            .get("release")
            .unwrap()
            .workers
            .into_iter()
            .map(|w| (w.chat_id, w.queue_position))
            .collect()
    }

    #[tokio::test]
    async fn archiving_an_unstarted_worker_persists_an_archived_chat() {
        use crate::{DocsStore, WorkspaceHostConfig};
        let dir = tempfile::tempdir().unwrap();
        let workspace = WorkspaceHost::open(
            Arc::new(DocsStore::open(dir.path()).unwrap()),
            WorkspaceHostConfig {
                device_id: "local".into(),
                device_name: "Test device".into(),
                platform: "linux".into(),
                org_id: "test-org".into(),
                user_id: "test-user".into(),
                edge: None,
                local_only: true,
            },
        )
        .unwrap();
        let store = Zappers::open(&dir.path().join("zappers"), "local").unwrap();
        store
            .command(command(
                "release",
                "create",
                ZapperAction::Create { config: config() },
            ))
            .unwrap();
        delegate(&store, "queued", "one");
        let zapper = store.get("release").unwrap();
        let worker = &zapper.workers[0];
        workspace
            .import_space_row(&zuse_protocol::Space {
                id: worker.space_id.clone(),
                device_id: worker.device_id.clone(),
                path: dir.path().to_string_lossy().into_owned(),
                name: None,
                git_detected: false,
                git_checked_at: None,
                checkout_id: None,
                repository_id: None,
                remote_repository: None,
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        assert!(workspace.chat("queued").unwrap().is_none());
        archive_unstarted_worker(&workspace, &zapper, "queued").unwrap();
        archive_unstarted_worker(&workspace, &zapper, "queued").unwrap();
        assert!(workspace.chat("queued").unwrap().unwrap().archived);
    }

    #[test]
    fn delegates_over_the_limit_queue_in_order_and_start_when_a_slot_frees() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        store
            .command(command(
                "release",
                "create",
                ZapperAction::Create {
                    config: ZapperConfig {
                        max_concurrent_workers: 1,
                        ..config()
                    },
                },
            ))
            .unwrap();
        delegate(&store, "a", "one");
        delegate(&store, "b", "two");
        // Not yet seen by the supervisor: nothing claims to be queued.
        assert_eq!(positions(&store), [("a".into(), None), ("b".into(), None)]);
        assert_eq!(pass(&store), ["a"]);
        delegate(&store, "c", "three");
        assert!(pass(&store).is_empty());
        assert_eq!(
            positions(&store),
            [
                ("a".into(), None),
                ("b".into(), Some(1)),
                ("c".into(), Some(2))
            ]
        );
        let zapper = store.get("release").unwrap();
        let label = |issue: &str| {
            let issue = zapper.issues.iter().find(|i| i.id == issue).unwrap();
            zapper.issue_status_label(issue)
        };
        // The stored status stays inProgress; what's shown is honest.
        assert_eq!(label("one"), "In progress");
        assert_eq!(label("two"), "Queued");
        assert_eq!(
            zapper.issues[1].details.status,
            ZapperIssueStatus::InProgress
        );
        let view = issues::coordinator_view(&zapper).unwrap();
        assert_eq!(view["issues"][1]["queued"], true);
        assert_eq!(view["issues"][1]["queuePosition"], 1);
        assert!(view["issues"][0].get("queued").is_none());

        // The running thread finishes: the next in line starts by itself.
        store.settle("a").unwrap();
        assert_eq!(pass(&store), ["b"]);
        assert_eq!(
            positions(&store),
            [
                ("a".into(), None),
                ("b".into(), None),
                ("c".into(), Some(1))
            ]
        );

        // Stopping a queued thread takes it out of the queue for good.
        store
            .command(command(
                "release",
                "stop-c",
                ZapperAction::StopWorker {
                    chat_id: "c".into(),
                },
            ))
            .unwrap();
        store.settle("b").unwrap();
        assert!(pass(&store).iter().all(|chat| chat != "c"));
        assert_eq!(positions(&store)[2], ("c".into(), None));
    }
}
