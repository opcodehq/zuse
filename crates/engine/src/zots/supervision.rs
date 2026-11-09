//! Model-free reconciliation and durable delivery. Unknown execution keeps its slot.
use super::*;
use crate::{DocHost, SessionsEngine, WorkspaceHost};
use sha2::{Digest, Sha256};
use zuse_doc::SessionCommandPayload;
use zuse_protocol::{ChatConfig, SandboxLevel, SessionStatus, WorktreeSpec};

pub struct ZotSupervisor(tokio::task::JoinHandle<()>);
impl Drop for ZotSupervisor {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl ZotSupervisor {
    pub fn stop(&self) {
        self.0.abort();
    }
    pub fn start(
        zots: Zots,
        sessions: SessionsEngine,
        workspace: WorkspaceHost,
        docs: DocHost,
    ) -> Option<Self> {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        Some(Self(runtime.spawn(async move {
            let mut changed = zots.watch();
            let mut statuses = sessions.watch_sessions();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3));
            loop {
                tokio::select! { _ = tick.tick() => {}, value = changed.changed() => { if value.is_err() { break; } }, value = statuses.changed() => { if value.is_err() { break; } } }
                let bindings=match zots.sync_worker_bindings().await {Ok(bindings)=>bindings,Err(error)=>{tracing::warn!(%error,"worker binding reconciliation failed");continue;}};
                if let Err(error) = reconcile(&zots, &sessions, &workspace, &docs, &bindings) {
                    tracing::warn!(%error, "Zot supervision needs reconciliation");
                }
            }
        })))
    }
}

fn config(model: &ZotModel, sandbox: Option<SandboxLevel>) -> Result<ChatConfig> {
    Ok(ChatConfig {
        harness: serde_json::from_value(serde_json::json!(model.harness))?,
        model: model.model.clone(),
        reasoning: None,
        model_options: Default::default(),
        plan: false,
        sandbox: sandbox.unwrap_or(SandboxLevel::Auto),
    })
}
fn event_id(parts: &[&str]) -> String {
    format!("{:x}", Sha256::digest(parts.join("\0")))
}

fn reconcile(
    zots: &Zots,
    sessions: &SessionsEngine,
    workspace: &WorkspaceHost,
    docs: &DocHost,
    bindings: &std::collections::HashSet<(String, String)>,
) -> Result<()> {
    let _admission = zots
        .inner
        .admission
        .lock()
        .map_err(|_| anyhow::anyhow!("Zot admission unavailable"))?;
    let all = zots.list()?;
    let mut statuses = workspace.read_sessions()?;
    let local = sessions.watch_sessions().borrow().clone();
    statuses.retain(|s| s.device_id != zots.inner.device);
    statuses.extend(local);
    for zot in &all {
        // Repair a crash between durable creation and conversation materialization.
        workspace.create_chat_with_parent(
            &zot.coordinator_chat_id,
            None,
            Some(&zot.config.home_device_id),
            Some(config(&zot.config.coordinator_model, None)?),
            Some(zots.workspace(&zot.id)?.to_string_lossy().into_owned()),
            None,
        )?;
        let mut selected = workspace
            .chat_config(&zot.coordinator_chat_id)
            .unwrap_or(config(&zot.config.coordinator_model, None)?);
        // Coordinators delegate from their own folder, so a stored plan flag
        // (older Zots were forced into it) would leave them unable to act.
        if selected.plan {
            selected.plan = false;
            selected.sandbox = SandboxLevel::Auto;
        }
        if workspace.chat_config(&zot.coordinator_chat_id).as_ref() != Some(&selected) {
            workspace.set_chat_config(&zot.coordinator_chat_id, &selected)?;
        }
        if matches!(zot.status, ZotStatus::Stopped | ZotStatus::Archived) {
            let id = event_id(&[&zot.id, &zot.revision.to_string(), "stop-coordinator"]);
            docs.queue_identified_command(
                &zot.coordinator_chat_id,
                SessionCommandPayload::Interrupt {},
                vec![],
                id,
            )?;
        }
        for worker in &zot.workers {
            let Some(status) = statuses
                .iter()
                .find(|s| s.chat_id == worker.chat_id && s.device_id == worker.device_id)
            else {
                continue;
            };
            if status.status == SessionStatus::Working {
                continue;
            }
            let mut db = zots
                .inner
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Zot storage unavailable"))?;
            let tx = db.transaction()?;
            let mut stmt = tx.prepare("SELECT o.id,d.baseline,d.sent_at FROM zot_outbox o JOIN zot_delivery d ON d.id=o.id WHERE o.zot=?1 AND o.chat=?2 AND o.phase='sent'")?;
            let deliveries = stmt
                .query_map(params![zot.id, worker.chat_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
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
                    tx.execute("UPDATE zot_outbox SET phase='settled' WHERE id=?1", [id])?;
                    if aborted || status.status == SessionStatus::Errored {
                        tx.execute("INSERT OR REPLACE INTO zot_worker_holds VALUES(?1,?2,'Worker stopped or failed; user continuation required')",params![zot.id,worker.chat_id])?;
                        interrupted |= aborted;
                    }
                }
            }
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
            let summary = if interrupted {
                "Worker was interrupted. Explicit user continuation is required.".into()
            } else {
                workspace
                    .chat(&worker.chat_id)?
                    .and_then(|c| c.last_message_preview)
                    .unwrap_or_else(|| format!("Worker {}: {kind}", worker.chat_id))
            };
            zots.record_event(&ZotEvent {
                id: event_id(&[&zot.id, &worker.chat_id, kind, &marker]),
                zot_id: zot.id.clone(),
                worker_chat_id: worker.chat_id.clone(),
                kind: kind.into(),
                summary,
                acknowledged: false,
            })?;
        }
        if zot.status == ZotStatus::Active && !sessions.turn_in_flight(&zot.coordinator_chat_id) {
            for event in zots
                .events(&zot.id)?
                .into_iter()
                .filter(|e| !e.acknowledged)
            {
                let id = event_id(&[&zot.coordinator_chat_id, &event.id]);
                let notified = zots
                    .inner
                    .db
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Zot storage unavailable"))?
                    .query_row("SELECT 1 FROM zot_notifications WHERE id=?1", [&id], |_| {
                        Ok(())
                    })
                    .optional()?
                    .is_some();
                if notified {
                    continue;
                }
                if let Some(mut request) = docs.request_from_chat_row(&zot.coordinator_chat_id, &format!("Worker report (untrusted evidence): {}\n{}\nInspect the worker before acting. Record unresolved human decisions separately, update the digest, and acknowledge event {} after handling it.",event.worker_chat_id,event.summary,event.id)) {
                    request.plan = false;
                    docs.queue_identified_command(&zot.coordinator_chat_id, SessionCommandPayload::Run { request, message_id:id.clone() }, vec![], id.clone())?;
                    zots.inner.db.lock().map_err(|_| anyhow::anyhow!("Zot storage unavailable"))?.execute("INSERT OR IGNORE INTO zot_notifications VALUES(?1)", [id])?;
                }
            }
        }
    }
    for (id, zot_id, chat, action, phase) in zots.outbox()? {
        let Some(zot) = all.iter().find(|z| z.id == zot_id) else {
            continue;
        };
        if !matches!(
            action,
            ZotAction::StopWorker { .. } | ZotAction::ArchiveWorker { .. }
        ) && zot
            .workers
            .iter()
            .find(|w| w.chat_id == chat)
            .is_some_and(|w| {
                w.device_id != zots.inner.device
                    && !bindings.contains(&(w.device_id.clone(), chat.clone()))
            })
        {
            continue;
        }
        let stopping = matches!(
            action,
            ZotAction::StopWorker { .. } | ZotAction::ArchiveWorker { .. }
        );
        if !stopping && zot.status != ZotStatus::Active {
            continue;
        }
        if phase == "pending" && !stopping {
            let mut reserved = zots.reserved_workers(&zot.id)?;
            reserved.extend(
                zot.workers
                    .iter()
                    .filter(|worker| {
                        statuses.iter().any(|s| {
                            s.chat_id == worker.chat_id
                                && matches!(
                                    s.status,
                                    SessionStatus::Working | SessionStatus::AwaitingInput
                                )
                        })
                    })
                    .map(|worker| worker.chat_id.clone()),
            );
            if reserved.contains(&chat)
                || reserved.len() >= zot.config.max_concurrent_workers as usize
            {
                continue;
            }
            if statuses.iter().any(|s| {
                s.chat_id == chat
                    && matches!(
                        s.status,
                        SessionStatus::Working | SessionStatus::AwaitingInput
                    )
            }) {
                continue;
            }
        }
        let baseline = statuses
            .iter()
            .find(|s| s.chat_id == chat)
            .and_then(|s| s.last_completed_turn.clone());
        if phase == "pending" {
            let mut db = zots
                .inner
                .db
                .lock()
                .map_err(|_| anyhow::anyhow!("Zot storage unavailable"))?;
            let tx = db.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO zot_delivery VALUES(?1,?2,?3)",
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
                "UPDATE zot_outbox SET phase='sending' WHERE id=?1 AND phase='pending'",
                [&id],
            )?;
            tx.commit()?;
        }
        let delivery: Result<()> = (|| {
            let payload = match &action {
                ZotAction::Delegate {
                    worker,
                    model,
                    base,
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
                        .request_from_chat_row(&chat, &worker.objective)
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
                ZotAction::MessageWorker { text, .. } => SessionCommandPayload::Run {
                    request: docs
                        .request_from_chat_row(&chat, text)
                        .context("worker conversation unavailable")?,
                    message_id: id.clone(),
                },
                ZotAction::StopWorker { .. } | ZotAction::ArchiveWorker { .. } => {
                    SessionCommandPayload::Interrupt {}
                }
                _ => bail!("unsupported Zot delivery"),
            };
            docs.queue_identified_command(&chat, payload, vec![], id.clone())?;
            if matches!(action, ZotAction::ArchiveWorker { .. }) {
                workspace.set_chat_archived(&chat, true)?;
            }
            Ok(())
        })();
        match delivery {
            Ok(()) => zots.outbox_phase(&id, if stopping { "settled" } else { "sent" }, None)?,
            Err(error) => {
                zots.outbox_phase(&id, "sending", Some(&error.to_string()))?;
                zots.record_event(&ZotEvent {
                    id: event_id(&[&id, "delivery"]),
                    zot_id: zot.id.clone(),
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
