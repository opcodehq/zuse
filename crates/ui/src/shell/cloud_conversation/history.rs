//! Scoped saved pages never become the execution watch's delta baseline.
use super::*;
use zuse_model::{TranscriptBaseline, TranscriptFrame, cloud::Head};

/// Capability failures are stable across RPC transports. Keep the original
/// fixed 401/403 display errors compatible with controllers predating typed errors.
pub(super) fn access_revoked(error: &zuse_rpc::RpcError) -> bool {
    matches!(error, zuse_rpc::RpcError::Capability(code) if code == "workspace_access_denied" || code == "workspace_not_found")
        || matches!(error, zuse_rpc::RpcError::Failed(message) if matches!(message.as_str(),
            "You do not have access to this workspace."
                | "Cloud could not verify your session. Reconnect in Settings → General."))
}

#[derive(Clone, PartialEq, Eq)]
struct Projection {
    workspace: String,
    session: String,
    generation: i64,
    epoch: String,
    schema: u32,
    format: String,
}
impl Projection {
    fn of(head: &Head) -> Self {
        Self {
            workspace: head.workspace_id.clone(),
            session: head.session_id.clone(),
            generation: head.runtime_generation,
            epoch: head.cursor.epoch.clone(),
            schema: head.schema_version,
            format: head.payload_format.clone(),
        }
    }
}
#[derive(Default)]
pub(super) struct State {
    pub head: Option<Head>,
    pub(super) provisional_read: Option<Task<()>>,
    pub(super) provisional_request: Option<String>,
    pub(super) fresh_received: bool,
    pub(super) provisional_painted: bool,
    pub older_read: Option<Task<()>>,
    live_floor: Option<u64>,
    retained_live: Option<Projection>,
}
impl State {
    pub fn stale(&self, head: &Head) -> bool {
        self.head.as_ref().is_some_and(|old| {
            head.runtime_generation < old.runtime_generation
                || (Projection::of(old) == Projection::of(head)
                    && head.cursor.version < old.cursor.version)
        })
    }
    pub fn latest(&mut self, mut head: Head) -> Option<(Head, bool)> {
        if head.runtime_generation <= 0
            || head.cursor.epoch.is_empty()
            || head.cursor.version < 0
            || head.older_message_sequence.is_some_and(|n| n <= 0)
        {
            return None;
        }
        if self
            .head
            .as_ref()
            .is_some_and(|old| head.runtime_generation < old.runtime_generation)
        {
            return None;
        }
        let changed = self
            .head
            .as_ref()
            .is_some_and(|old| Projection::of(old) != Projection::of(&head));
        if let Some(old) = &self.head {
            if !changed && head.cursor.version < old.cursor.version {
                return None;
            }
            if !changed && self.retains_complete_live() && head.cursor.version == old.cursor.version
            {
                // The last acknowledged API cursor may predate a live stream.
                // An equal-version replay cannot replace that retained snapshot.
                head.transcript = old.transcript.clone();
                head.older_message_sequence = old.older_message_sequence;
            } else if !changed {
                let contiguous = zuse_model::cloud::merge_loaded_prefix(old, &mut head);
                if !contiguous {
                    self.retained_live = None;
                }
            }
        }
        if changed {
            self.older_read = None;
            self.live_floor = None;
            self.retained_live = None;
        }
        self.head = Some(head.clone());
        Some((head, changed))
    }
    pub fn start_live(&mut self, revision: u64) {
        self.live_floor = Some(revision);
    }
    pub fn observe_live(&mut self, state: &AppState) {
        if state.engine().is_some()
            && state.transcript_replayed
            && self
                .live_floor
                .is_some_and(|floor| state.transcript_revision > floor)
        {
            self.retained_live = self.head.as_ref().map(Projection::of);
        }
    }
    pub fn materialize_live(&mut self, state: &AppState) {
        if !self.retains_complete_live() {
            return;
        }
        if let Some(head) = &mut self.head {
            let rows = state.transcript.clone();
            head.transcript.replay_baseline = Some(TranscriptBaseline::capture(&rows));
            head.transcript.frame = TranscriptFrame::Reset { reset: rows.into() };
            head.older_message_sequence = None;
        }
    }
    pub fn retains_complete_live(&self) -> bool {
        self.head
            .as_ref()
            .is_some_and(|head| self.retained_live.as_ref() == Some(&Projection::of(head)))
    }
    fn matches_page_request(&self, expected: &Head) -> bool {
        self.head.as_ref().is_some_and(|head| {
            Projection::of(head) == Projection::of(expected)
                && head.older_message_sequence == expected.older_message_sequence
        })
    }
    fn older(&mut self, expected: &Head, page: Head) -> Result<Head, ()> {
        let current = self.head.as_ref().ok_or(())?;
        let before = expected.older_message_sequence.ok_or(())?;
        if Projection::of(current) != Projection::of(expected)
            || Projection::of(&page) != Projection::of(expected)
            || current.older_message_sequence != Some(before)
            || page.cursor.version < expected.cursor.version
        {
            return Err(());
        }
        let (
            TranscriptFrame::Reset { reset: incoming },
            TranscriptFrame::Reset { reset: existing },
        ) = (&page.transcript.frame, &current.transcript.frame)
        else {
            return Err(());
        };
        let next = page.older_message_sequence.unwrap_or(0);
        if incoming.is_empty()
            || incoming.len() > 64
            || next < 0
            || next >= before
            || next.checked_add(incoming.len() as i64) != Some(before)
        {
            return Err(());
        }
        let mut ids = std::collections::HashSet::new();
        if !incoming
            .iter()
            .chain(existing.iter())
            .all(|row| ids.insert(row.id.as_str()))
        {
            return Err(());
        }
        let mut rows = incoming.to_vec();
        rows.extend(existing.iter().cloned());
        let mut head = current.clone();
        head.transcript.replay_baseline = Some(TranscriptBaseline::capture(&rows));
        head.transcript.frame = TranscriptFrame::Reset { reset: rows.into() };
        head.older_message_sequence = page.older_message_sequence;
        self.head = Some(head.clone());
        Ok(head)
    }
}
impl CloudConversation {
    /// Platform-protected saved pages can paint while a distinct fresh read is
    /// pending. They carry no authority for controls, choices or attachment.
    /// Runs as soon as the conversation opens, without waiting for the account
    /// check: the engine only decrypts a page saved for the signed-in account,
    /// and a revocation clears whatever painted.
    pub(super) fn read_provisional_history(&mut self, cx: &mut Context<Self>) {
        if self.access_revoked
            || self.history.fresh_received
            || self.history.provisional_read.is_some()
            || self.state.read(cx).engine().is_some()
        {
            return;
        }
        let target = self.target.clone();
        let request = uuid::Uuid::new_v4().to_string();
        self.history.provisional_request = Some(request.clone());
        let mut params = self.params();
        params["cacheOnly"] = json!(true);
        self.history.provisional_read = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(methods::CLOUD_HISTORY, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.history.provisional_request.as_deref() != Some(request.as_str()) {
                    return;
                }
                this.history.provisional_read = None;
                this.history.provisional_request = None;
                if this.access_revoked
                    || this.history.fresh_received
                    || this.state.read(cx).engine().is_some()
                    || !this.state.read(cx).transcript.is_empty()
                    || this.target.subject != target.subject
                    || this.target.organization != target.organization
                    || this.target.workspace != target.workspace
                    || this.target.initial_session != target.initial_session
                {
                    return;
                }
                let Ok(value) = result else {
                    return;
                };
                if value["provisional"] != true
                    || value["accountId"] != target.subject
                    || value["organizationId"].as_str() != target.organization.as_deref()
                {
                    return;
                }
                let Ok(head) = serde_json::from_value::<Head>(value["head"].clone()) else {
                    return;
                };
                if head.workspace_id != target.workspace
                    || head.session_id != target.initial_session
                {
                    return;
                }
                if !matches!(&head.transcript.frame, TranscriptFrame::Reset { .. }) {
                    return;
                }
                let Some((head, _)) = this.history.latest(head) else {
                    return;
                };
                this.history.provisional_painted = true;
                // Unlike a fresh saved head, a provisional head must not apply
                // cached queue/config/admissions or become a live watch baseline.
                this.state.update(cx, |state, cx| {
                    state.chats = head.conversations.clone();
                    if !state.chats.iter().any(|chat| chat.id == head.chat.id) {
                        state.chats.push(head.chat.clone());
                    }
                    state.selected_chat = Some(head.session_id.clone());
                    if let TranscriptFrame::Reset { reset } = &head.transcript.frame {
                        state.apply_transcript(reset.to_vec());
                    }
                    cx.notify();
                });
                cx.notify();
            });
        }));
    }
    pub(super) fn load_older(&mut self, cx: &mut Context<Self>) {
        if !self.authorized
            || self.access_revoked
            || self.state.read(cx).engine().is_some()
            || self.history.retains_complete_live()
            || self.history.older_read.is_some()
            || self.history_read.is_some()
        {
            return;
        }
        let Some(expected) = self.history.head.clone() else {
            return;
        };
        let Some(before) = expected.older_message_sequence else {
            return;
        };
        let target = self.target.clone();
        let mut params = self.params();
        params["beforeSequence"] = json!(before);
        params["cursor"] = json!(expected.cursor);
        params["incremental"] = json!(true);
        self.error = None;
        self.history.older_read = Some(cx.spawn(async move |this,cx| {
            let result = target.controller.client().call_scoped(methods::CLOUD_HISTORY, params).await;
            let _ = this.update(cx,|this,cx| {
                if !this.authorized || this.access_revoked
                    || this.target.subject != target.subject
                    || this.target.organization != target.organization
                    || this.target.workspace != target.workspace
                    || this.target.initial_session != target.initial_session { return; }
                // Access denial belongs to this task's authority, even when a
                // newer projection or live attachment superseded this page.
                if let Some(error) = result.as_ref().err().filter(|error| access_revoked(error)) {
                    this.revoke_for_history_error(error, cx);
                    return;
                }
                if !this.history.matches_page_request(&expected) { return; }
                this.history.older_read = None;
                if this.state.read(cx).engine().is_some() || this.history.retains_complete_live() { return; }
                match result {
                    Ok(value) if value["accountId"] == target.subject
                        && value["organizationId"].as_str() == target.organization.as_deref() => {
                        let accepted = serde_json::from_value::<Head>(value["head"].clone()).ok()
                            .and_then(|page| this.history.older(&expected,page).ok());
                        if let Some(head) = accepted { this.project_saved_history(&head,cx); }
                        else { this.error = Some("Earlier history changed. Refresh this conversation before trying again.".into()); }
                    }
                    Ok(_) => this.revoke(cx),
                    Err(_) => this.error = Some("Earlier history could not be loaded. Try again when connected.".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn project_saved_history(&mut self, head: &Head, cx: &mut Context<Self>) {
        if self.state.read(cx).engine().is_some() {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.chats = head.conversations.clone();
            if !state.chats.iter().any(|chat| chat.id == head.chat.id) {
                state.chats.push(head.chat.clone());
            }
            state.selected_chat = Some(head.session_id.clone());
            if let Ok(queue) =
                serde_json::from_value::<Vec<zuse_doc::QueuedMessage>>(head.queue.clone())
            {
                state.apply_queue(queue);
            }
            let _ = state.receive_transcript_update(head.transcript.clone(), cx);
            // This head already includes any verified retained live prefix.
            // Higher API cursors update its tail; equal stale snapshots cannot.
            // New durable admissions still get a scoped echo.
            let rows = crate::cloud_history::CloudHistory::admission_rows(
                head,
                self.admissions.as_array().map(Vec::as_slice).unwrap_or(&[]),
                state.transcript.clone(),
            );
            state.apply_transcript(rows);
            cx.notify();
        });
    }
    pub(super) fn render_older_history(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.authorized
            || self.access_revoked
            || self.state.read(cx).engine().is_some()
            || self.history.retains_complete_live()
            || self.history.head.as_ref()?.older_message_sequence.is_none()
        {
            return None;
        }
        let loading = self.history.older_read.is_some();
        let enabled = !loading && self.history_read.is_none();
        let theme = Theme::of(cx).clone();
        Some(
            popover::btn_ghost(
                &theme,
                if loading {
                    "Loading earlier messages…"
                } else {
                    "Load earlier messages"
                },
                "native-cloud-history-older",
            )
            .id("native-cloud-history-older")
            .debug_selector(|| "native-cloud-history-older".into())
            .opacity(if enabled { 1. } else { 0.5 })
            .on_click(cx.listener(move |this, _, _, cx| {
                if enabled {
                    this.load_older(cx);
                }
            }))
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn head(ids: &[&str], before: Option<i64>) -> Head {
        serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":"fixture","workspaceId":"task","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"epoch","version":2},"chat":{"id":"session","deviceId":"cloud:task","archived":false,"createdAt":"2026-10-31T00:00:00Z"},"conversations":[],"transcript":{"reset":ids.iter().map(|id|json!({"id":id,"role":"assistant","parts":[{"id":id,"kind":"text","text":id}],"createdAt":1,"deviceId":"cloud:task"})).collect::<Vec<_>>()},"queue":[],"queuePaused":false,"olderMessageSequence":before})).unwrap()
    }
    #[test]
    fn older_page_requires_same_projection_cursor_and_contiguous_unique_range() {
        let current = head(&["tail"], Some(2));
        let page = head(&["one", "two"], None);
        for mutation in [
            "workspace",
            "session",
            "generation",
            "epoch",
            "version",
            "cursor",
            "gap",
            "duplicate",
            "overlap",
        ] {
            let mut state = State::default();
            state.latest(current.clone()).unwrap();
            let mut bad = page.clone();
            let mut expected = current.clone();
            match mutation {
                "workspace" => bad.workspace_id = "other".into(),
                "session" => bad.session_id = "other".into(),
                "generation" => bad.runtime_generation += 1,
                "epoch" => bad.cursor.epoch = "other".into(),
                "version" => bad.cursor.version = 1,
                "cursor" => expected.older_message_sequence = Some(3),
                "gap" => bad = head(&["one"], None),
                "duplicate" => bad = head(&["one", "one"], None),
                "overlap" => bad = head(&["one", "tail"], None),
                _ => unreachable!(),
            }
            if mutation == "cursor" {
                assert!(!state.matches_page_request(&expected));
            }
            assert!(state.older(&expected, bad).is_err(), "{mutation}");
            assert_eq!(state.head.unwrap().older_message_sequence, Some(2));
        }
        let mut state = State::default();
        state.latest(current.clone()).unwrap();
        let merged = state.older(&current, page).unwrap();
        assert!(merged.older_message_sequence.is_none());
        let TranscriptFrame::Reset { reset } = merged.transcript.frame else {
            panic!("reset")
        };
        assert_eq!(
            reset.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["one", "two", "tail"]
        );
    }
    #[test]
    fn latest_projection_changes_clear_old_ranges_and_reject_generation_or_version_rollback() {
        let mut state = State::default();
        let mut current = head(&["tail"], Some(2));
        current.runtime_generation = 2;
        state.latest(current.clone()).unwrap();
        let mut stale = current.clone();
        stale.runtime_generation = 1;
        assert!(state.latest(stale).is_none());
        let mut stale = current.clone();
        stale.cursor.version = 1;
        assert!(state.latest(stale).is_none());
        state.retained_live = Some(Projection::of(&current));
        let mut reset = current;
        reset.cursor.epoch = "new-epoch".into();
        reset.cursor.version = 0;
        assert!(state.latest(reset).unwrap().1);
        assert!(!state.retains_complete_live());
    }
}
