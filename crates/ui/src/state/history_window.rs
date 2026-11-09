//! Native presentation windows. Provider and durable document history stay complete.
use super::*;

#[derive(Clone, Default)]
pub(super) struct NativeHistory {
    pub generation: uuid::Uuid,
    pub loading: bool,
    has_more: bool,
    next_anchor: Option<String>,
}

#[derive(Clone)]
pub(super) struct HistoryFrame {
    pub resumed: bool,
    has_more: Option<bool>,
    next_anchor: Option<String>,
}
impl HistoryFrame {
    pub fn parse(value: &serde_json::Value, resume_requested: bool) -> Self {
        Self {
            resumed: resume_requested && value["historyResumed"] == true,
            has_more: value["historyHasMore"].as_bool(),
            next_anchor: value["historyNextAnchor"].as_str().map(str::to_owned),
        }
    }
    pub fn validate_opening(&self, opening: bool, frame: &TranscriptFrame) -> Result<(), String> {
        if opening && !self.resumed && !matches!(frame, TranscriptFrame::Reset { .. }) {
            Err("host omitted the initial transcript snapshot".into())
        } else {
            Ok(())
        }
    }
}
impl NativeHistory {
    pub fn receive(&mut self, frame: &HistoryFrame, opening: bool) -> bool {
        let before = (self.loading, self.has_more, self.next_anchor.clone());
        self.loading = false;
        if opening || frame.resumed || frame.has_more.is_some() {
            self.has_more = frame.has_more.unwrap_or(false);
            self.next_anchor = frame.next_anchor.clone();
        }
        before != (self.loading, self.has_more, self.next_anchor.clone())
    }
}

pub(super) struct WatchSeed {
    history: NativeHistory,
    first: Option<String>,
    entries: Vec<SessionMessageEntry>,
    prepared: Option<Arc<crate::transcript::PreparedTranscript>>,
}
impl WatchSeed {
    pub fn capture(
        history: &NativeHistory,
        entries: &[SessionMessageEntry],
        prepared: Option<Arc<crate::transcript::PreparedTranscript>>,
    ) -> Self {
        // Copying and hashing a retained whale on the UI thread would defeat
        // paging. Normal opening windows are small; large views get a reset.
        let resumable = entries.len() <= 2048
            && entries
                .iter()
                .map(|e| e.id.len() + e.parts.iter().map(|p| p.byte_len()).sum::<usize>())
                .sum::<usize>()
                <= 2 * 1024 * 1024;
        Self {
            history: history.clone(),
            first: entries.first().map(|e| e.id.clone()),
            entries: if resumable {
                entries.to_vec()
            } else {
                Vec::new()
            },
            prepared: if resumable { prepared } else { None },
        }
    }
    /// Hashing and construction of the preparation mirror run on the worker.
    pub fn prepare(
        self,
        chat_id: &str,
    ) -> (
        serde_json::Value,
        crate::transcript::TranscriptPreparation,
        bool,
    ) {
        let resume = zuse_model::TranscriptResume::capture(&self.entries);
        let requested = resume.is_some();
        let params = serde_json::json!({
            "chatId": chat_id,
            "openingTail": true, // older hosts keep their existing preview/full behavior
            "tailLimit": 32,
            "tailBytes": 64 * 1024,
            "tailAnchor": if self.history.loading { self.history.next_anchor } else { self.first },
            "resume": resume,
        });
        (
            params,
            crate::transcript::TranscriptPreparation::from_retained(self.entries, self.prepared),
            requested,
        )
    }
}

impl AppState {
    pub(crate) fn native_history_status(&self, doc: Option<&str>) -> Option<bool> {
        let history = match doc {
            Some(doc) => self.sub_history.get(doc)?,
            None => &self.transcript_history,
        };
        history.has_more.then_some(history.loading)
    }

    pub(crate) fn load_earlier_native(&mut self, doc: Option<&str>, cx: &mut Context<Self>) {
        let Some(handle) = self.engine.clone() else {
            return;
        };
        match doc {
            Some(doc) => {
                let Some(history) = self.sub_history.get_mut(doc) else {
                    return;
                };
                if !history.has_more || history.loading {
                    return;
                }
                history.loading = true;
                self.sub_watch_tasks.remove(doc);
                self.watch_subagent_doc(doc.to_owned(), cx);
            }
            None => {
                let Some(chat_id) = self.selected_chat.clone() else {
                    return;
                };
                let history = &mut self.transcript_history;
                if !history.has_more || history.loading {
                    return;
                }
                history.loading = true;
                history.generation = uuid::Uuid::new_v4();
                self.transcript_task = Some(super::spawn_transcript_watch(
                    cx,
                    handle,
                    chat_id,
                    history.generation,
                ));
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(id: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: zuse_doc::MessageRole::User,
            parts: vec![zuse_doc::MessagePart::Text {
                id: "text".into(),
                text: "retained body".into(),
            }],
            created_at: 0,
            device_id: "test".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }
    #[test]
    fn native_open_page_reconnect_and_legacy_metadata() {
        let mut history = NativeHistory::default();
        let (params, _, resumed) = WatchSeed::capture(&history, &[], None).prepare("chat");
        assert_eq!(params["tailLimit"], 32);
        assert_eq!(params["tailBytes"], 65536);
        assert!(params["tailAnchor"].is_null());
        assert!(!resumed);
        history.receive(
            &HistoryFrame::parse(
                &serde_json::json!({
                    "historyHasMore":true,"historyNextAnchor":"older"
                }),
                false,
            ),
            true,
        );
        history.loading = true;
        let entries = vec![entry("current")];
        let (params, mut worker, resumed) =
            WatchSeed::capture(&history, &entries, None).prepare("chat");
        assert_eq!(params["tailAnchor"], "older");
        assert!(resumed);
        let hint: zuse_model::TranscriptResume =
            serde_json::from_value(params["resume"].clone()).unwrap();
        let expanded = vec![entry("older"), entry("current"), entry("new")];
        let frame = hint.frame(&expanded).unwrap();
        let update: zuse_doc::TranscriptUpdate =
            serde_json::from_value(serde_json::to_value(frame).unwrap()).unwrap();
        // A resumed preparation mirror must already have the retained middle.
        worker.prepare(&update).unwrap();
        history.receive(&HistoryFrame::parse(&serde_json::json!({"historyResumed":true,"historyHasMore":true,"historyNextAnchor":"even-older"}), true), true);
        assert!(!history.loading);
        let (params, _, _) = WatchSeed::capture(&history, &expanded, None).prepare("chat");
        assert_eq!(
            params["tailAnchor"], "older",
            "reconnect preserves the expanded window"
        );
        history.receive(&HistoryFrame::parse(&serde_json::json!({}), false), false);
        assert!(
            history.has_more,
            "ordinary deltas preserve pagination metadata"
        );
        history.receive(&HistoryFrame::parse(&serde_json::json!({}), false), true);
        assert!(
            !history.has_more,
            "legacy full reset clears paging metadata"
        );
    }
    #[test]
    fn native_rejects_unrequested_resume_and_does_not_copy_whales() {
        let delta = TranscriptFrame::Delta {
            upsert: vec![],
            append: vec![],
            remove: vec![],
            count: 0,
        };
        assert!(
            HistoryFrame::parse(&serde_json::json!({"historyResumed":true}), false)
                .validate_opening(true, &delta)
                .is_err()
        );
        assert!(
            HistoryFrame::parse(&serde_json::json!({"historyResumed":true}), true)
                .validate_opening(true, &delta)
                .is_ok()
        );
        let mut whale = entry("huge");
        whale.parts = vec![zuse_doc::MessagePart::Text {
            id: "text".into(),
            text: "x".repeat(3 * 1024 * 1024),
        }];
        let (params, _, requested) =
            WatchSeed::capture(&NativeHistory::default(), &[whale], None).prepare("chat");
        assert!(!requested);
        assert!(params["resume"].is_null());
        assert_eq!(
            params["tailAnchor"], "huge",
            "reset retains the loaded range"
        );
    }
    #[gpui::test]
    fn native_cached_window_retains_paging_and_frozen_agents_drop_it(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            state.select_chat(Some("chat".into()), cx);
            state.apply_transcript(vec![entry("recent")]);
            state.transcript_history.receive(
                &HistoryFrame::parse(
                    &serde_json::json!({
                        "historyHasMore":true,"historyNextAnchor":"older"
                    }),
                    false,
                ),
                true,
            );
            state.select_chat(None, cx);
            state.select_chat(Some("chat".into()), cx);
            assert_eq!(state.native_history_status(None), Some(false));
            assert_eq!(
                state.transcript_history.next_anchor.as_deref(),
                Some("older")
            );
            state
                .sub_history
                .insert("agent".into(), state.transcript_history.clone());
            assert_eq!(state.native_history_status(Some("agent")), Some(false));
            state.set_subagent_snapshot("agent".into(), vec![entry("frozen")]);
            assert_eq!(state.native_history_status(Some("agent")), None);
            state.prepare_runtime_replacement(cx);
            assert_eq!(state.native_history_status(None), None);
            assert!(state.sub_history.is_empty());
        });
    }

    #[tokio::test]
    async fn native_paired_history_open_page_resume_and_live_update() {
        use std::time::Duration;
        let root = tempfile::tempdir().unwrap();
        let core = zuse_engine::EngineCore::assemble(
            root.path(),
            Arc::new(zuse_engine::default_registry()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let doc_id = "native-history";
        let doc = core.doc_host.open(doc_id).unwrap();
        let source = doc.doc_arc();
        for i in 0..1500 {
            let mut row = entry(&format!("message-{i}"));
            row.parts = vec![zuse_doc::MessagePart::Text {
                id: "text".into(),
                text: format!("Message {i}: {}", "café 🦀 native history ".repeat(40)),
            }];
            source.push_message(&row).unwrap();
        }
        let expected = source.read_entries().unwrap();
        let (_, invitation) = core.pairings.create("Native history test").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = zuse_remote::PairedTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            invitation,
        )
        .unwrap();
        let stop = tokio_util::sync::CancellationToken::new();
        let server = zuse_serve::BrowserServer::new(core.rpc_service(), "x".repeat(64), None)
            .unwrap()
            .with_pairings(core.pairings.clone(), core.device_id.clone());
        let server_stop = stop.clone();
        let server_task = tokio::spawn(server.serve_with_shutdown(
            listener,
            root.path().into(),
            None,
            server_stop.cancelled_owned(),
        ));
        // This is the native desktop connection, including pairing, encryption,
        // LZ4 negotiation and persistent session ACKs, rather than a browser socket.
        let engine = super::super::remote_connection::paired(target, root.path())
            .await
            .unwrap();
        let mut history = NativeHistory::default();
        let (params, mut worker, _) = WatchSeed::capture(&history, &[], None).prepare(doc_id);
        let mut watch = engine
            .client()
            .subscribe_checked(methods::WATCH_DOC_MESSAGES, params)
            .await
            .unwrap();
        let opening = tokio::time::timeout(Duration::from_secs(5), watch.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(opening["historyTotal"], 1500);
        assert_eq!(opening["historyRows"], 32);
        let update: zuse_doc::TranscriptUpdate = serde_json::from_value(opening.clone()).unwrap();
        let mut retained = Vec::new();
        zuse_doc::apply_transcript_frame(&mut retained, update.frame.clone()).unwrap();
        let prepared = worker.prepare(&update).unwrap();
        assert_eq!(retained, &expected[1468..]);
        history.receive(&HistoryFrame::parse(&opening, false), true);
        drop(watch);
        history.loading = true;
        let (params, mut worker, _) =
            WatchSeed::capture(&history, &retained, Some(prepared)).prepare(doc_id);
        let mut watch = engine
            .client()
            .subscribe_checked(methods::WATCH_DOC_MESSAGES, params)
            .await
            .unwrap();
        let older = tokio::time::timeout(Duration::from_secs(5), watch.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(older["historyResumed"], true);
        let update: zuse_doc::TranscriptUpdate = serde_json::from_value(older.clone()).unwrap();
        zuse_doc::apply_transcript_frame(&mut retained, update.frame.clone()).unwrap();
        let prepared = worker.prepare(&update).unwrap();
        assert_eq!(retained, &expected[1436..]);
        history.receive(&HistoryFrame::parse(&older, true), true);
        drop(watch);
        let (params, mut worker, _) =
            WatchSeed::capture(&history, &retained, Some(prepared)).prepare(doc_id);
        let mut watch = engine
            .client()
            .subscribe_checked(methods::WATCH_DOC_MESSAGES, params)
            .await
            .unwrap();
        let resumed = tokio::time::timeout(Duration::from_secs(5), watch.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resumed["historyResumed"], true);
        assert!(
            resumed["upsert"].as_array().unwrap().is_empty(),
            "unchanged history bodies are skipped"
        );
        let update: zuse_doc::TranscriptUpdate = serde_json::from_value(resumed).unwrap();
        worker.prepare(&update).unwrap();
        let live = entry("live");
        source.push_message(&live).unwrap();
        let next = tokio::time::timeout(Duration::from_secs(5), watch.recv())
            .await
            .unwrap()
            .unwrap();
        let update: zuse_doc::TranscriptUpdate = serde_json::from_value(next).unwrap();
        worker.prepare(&update).unwrap();
        zuse_doc::apply_transcript_frame(&mut retained, update.frame).unwrap();
        assert_eq!(retained.last(), Some(&live));
        assert_eq!(
            source.read_entries().unwrap().len(),
            1501,
            "durable history stays complete"
        );
        drop(watch);
        engine.shutdown().await;
        stop.cancel();
        server_task.await.unwrap().unwrap();
        core.doc_host.shutdown_workers().await;
    }
}
