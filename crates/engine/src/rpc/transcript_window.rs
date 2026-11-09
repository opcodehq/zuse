//! Presentation windows bound initial network traffic, never durable history.
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct History {
    pub history_has_more: bool,
    pub history_total: usize,
    pub history_rows: usize,
    pub history_next_anchor: Option<String>,
}

#[derive(serde::Serialize)]
pub(super) struct Envelope {
    #[serde(flatten)]
    pub update: zuse_doc::TranscriptUpdate,
    #[serde(rename = "historyResumed", skip_serializing_if = "std::ops::Not::not")]
    pub history_resumed: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub history: Option<History>,
}

pub(super) struct Window {
    rows: usize,
    bytes: usize,
    anchor: Option<String>,
    anchor_index: Option<usize>,
    previous_anchor: Option<String>,
    resume: Option<zuse_model::TranscriptResume>,
}
impl Window {
    pub fn new(rows: usize, bytes: usize) -> Self {
        Self {
            rows: rows.max(1),
            bytes: bytes.max(1),
            anchor: None,
            anchor_index: None,
            previous_anchor: None,
            resume: None,
        }
    }
    pub fn with_anchor(mut self, anchor: Option<String>) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn with_resume(mut self, resume: Option<zuse_model::TranscriptResume>) -> Self {
        self.resume = resume;
        self
    }
    pub fn resume_frame(
        &mut self,
        entries: &[zuse_doc::SessionMessageEntry],
    ) -> Option<zuse_doc::TranscriptFrame> {
        self.resume.take()?.frame(entries)
    }

    pub fn project(
        &mut self,
        source: crate::doc_host::TranscriptSnapshot,
    ) -> (crate::doc_host::TranscriptSnapshot, History) {
        let entries = &source.entries;
        // Keep the first visible entry anchored while new replies arrive.
        // A rolling tail would remove content underneath someone scrolling.
        let cached = self.anchor_index.filter(|&index| {
            entries
                .get(index)
                .is_some_and(|entry| Some(&entry.id) == self.anchor.as_ref())
        });
        let start = cached
            .or_else(|| {
                self.anchor
                    .as_ref()
                    .and_then(|id| entries.iter().position(|entry| &entry.id == id))
            })
            .unwrap_or_else(|| {
                let mut start = entries.len();
                let mut bytes = 0usize;
                for entry in entries.iter().rev().take(self.rows) {
                    let size = serde_json::to_vec(entry).map_or(0, |value| value.len());
                    if start < entries.len() && bytes.saturating_add(size) > self.bytes {
                        break;
                    }
                    bytes = bytes.saturating_add(size);
                    start -= 1;
                }
                start
            });
        self.anchor = entries.get(start).map(|entry| entry.id.clone());
        self.anchor_index = Some(start);
        let mut previous = start;
        let mut previous_bytes = 0usize;
        if cached.is_none() {
            for entry in entries[..start].iter().rev().take(32) {
                let size = serde_json::to_vec(entry).map_or(0, |value| value.len());
                if previous < start && previous_bytes.saturating_add(size) > 64 * 1024 {
                    break;
                }
                previous_bytes = previous_bytes.saturating_add(size);
                previous -= 1;
            }
            self.previous_anchor = (start > 0).then(|| entries[previous].id.clone());
        }
        let history = History {
            history_has_more: start > 0,
            history_total: entries.len(),
            history_rows: entries.len() - start,
            history_next_anchor: self.previous_anchor.clone(),
        };
        let entries = if start == 0 {
            source.entries
        } else {
            Arc::new(entries[start..].to_vec())
        };
        (
            crate::doc_host::TranscriptSnapshot {
                entries,
                replay_baseline: source.replay_baseline,
            },
            history,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::{doc_message_envelopes, watch};
    use super::*;
    use futures::StreamExt;

    fn snapshot(count: usize) -> crate::doc_host::TranscriptSnapshot {
        let entries: Vec<_> = (0..count)
            .map(|index| zuse_doc::SessionMessageEntry {
                id: format!("message-{index}"),
                role: zuse_doc::MessageRole::Assistant,
                parts: vec![zuse_doc::MessagePart::Text {
                    id: "text".into(),
                    text: "本🦀 fixture ".repeat(100),
                }],
                created_at: index as i64,
                device_id: "host".into(),
                status: Some(zuse_doc::MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })
            .collect();
        let baseline = zuse_doc::TranscriptBaseline::capture(&entries);
        crate::doc_host::TranscriptSnapshot {
            entries: Arc::new(entries),
            replay_baseline: Arc::new(baseline),
        }
    }

    #[tokio::test]
    async fn large_history_is_bounded_live_appends_keep_the_anchor_and_cancel_releases_the_watch() {
        let full = snapshot(3000);
        let full_bytes = serde_json::to_vec(&*full.entries).unwrap().len();
        let doc = Arc::new(zuse_doc::SessionDoc::init("bounded").unwrap());
        let (tx, rx) = watch::channel(full.clone());
        let mut stream = doc_message_envelopes(rx, doc, Some(Window::new(32, 64 * 1024)));
        let first = stream.next().await.unwrap();
        let wire = serde_json::to_vec(&first).unwrap();
        let mut visible = Vec::new();
        zuse_doc::apply_transcript_frame(&mut visible, first.update.frame).unwrap();
        assert_eq!(visible.len(), 32);
        assert!(first.history.unwrap().history_has_more);
        assert!(wire.len() < 64 * 1024);
        assert!(
            wire.len() * 50 < full_bytes,
            "initial chat bytes: {} versus {full_bytes}",
            wire.len()
        );
        let anchor = visible[0].id.clone();
        tx.send_replace(snapshot(3040));
        let next = stream.next().await.unwrap();
        assert_eq!(next.history.unwrap().history_total, 3040);
        assert!(next.update.replay_baseline.as_ref().unwrap().entries.len() <= 72);
        zuse_doc::apply_transcript_frame(&mut visible, next.update.frame).unwrap();
        assert_eq!(visible.len(), 72);
        assert_eq!(
            visible[0].id, anchor,
            "new turns must not shift old content out of the window"
        );
        assert_eq!(
            full.entries.len(),
            3000,
            "presentation never mutates durable history"
        );
        drop(stream);
        assert!(tx.is_closed());
        eprintln!(
            "chat initial wire: {} bytes, full history: {full_bytes} bytes",
            wire.len()
        );
    }

    #[test]
    fn byte_budget_and_explicit_expansion_preserve_full_utf8_messages() {
        let full = snapshot(100);
        let mut small = Window::new(32, 4096);
        let (initial, history) = small.project(full.clone());
        assert!(initial.entries.len() < 32 && history.history_has_more);
        assert_eq!(
            initial.entries.last().unwrap(),
            full.entries.last().unwrap()
        );
        let (all, history) = Window::new(100, usize::MAX).project(full.clone());
        assert!(!history.history_has_more);
        assert_eq!(all.entries, full.entries);
        let (one, _) = Window::new(32, 1).project(full.clone());
        assert_eq!(one.entries.len(), 1);
        assert_eq!(
            one.entries[0],
            *full.entries.last().unwrap(),
            "never truncate a large last message"
        );
    }

    #[test]
    fn earlier_page_anchors_reach_every_message_including_oversized_entries() {
        let mut full = snapshot(100);
        let entries = Arc::make_mut(&mut full.entries);
        entries[50].parts = vec![zuse_doc::MessagePart::Text {
            id: "large".into(),
            text: "large 🦀 ".repeat(20_000),
        }];
        let mut anchor = None;
        let mut previous_count = 0;
        loop {
            let (visible, history) = Window::new(32, 64 * 1024)
                .with_anchor(anchor)
                .project(full.clone());
            assert!(visible.entries.len() > previous_count);
            assert_eq!(
                visible.entries.as_slice(),
                &full.entries[100 - visible.entries.len()..]
            );
            if !history.history_has_more {
                assert_eq!(visible.entries, full.entries);
                assert!(history.history_next_anchor.is_none());
                break;
            }
            previous_count = visible.entries.len();
            anchor = history.history_next_anchor;
            assert!(anchor.is_some());
        }
    }

    #[tokio::test]
    async fn live_text_omits_unchanged_history_metadata() {
        let full = snapshot(100);
        let doc = Arc::new(zuse_doc::SessionDoc::init("metadata").unwrap());
        let (tx, rx) = watch::channel(full.clone());
        let mut stream = doc_message_envelopes(rx, doc, Some(Window::new(32, 64 * 1024)));
        assert!(
            stream
                .next()
                .await
                .unwrap()
                .history
                .unwrap()
                .history_has_more
        );
        let mut changed = full;
        let entry = Arc::make_mut(&mut changed.entries).last_mut().unwrap();
        let zuse_doc::MessagePart::Text { text, .. } = &mut entry.parts[0] else {
            unreachable!()
        };
        text.push_str(" live text");
        tx.send_replace(changed);
        let update = stream.next().await.unwrap();
        assert!(update.history.is_none());
        assert!(
            !serde_json::to_string(&update)
                .unwrap()
                .contains("historyHasMore")
        );
    }
    #[tokio::test]
    async fn older_page_sends_only_missing_rows_and_reconnect_validates_cached_bodies() {
        let full = snapshot(100);
        let mut cached = full.entries[68..].to_vec();
        let hint = zuse_model::TranscriptResume::capture(&cached).unwrap();
        let doc = Arc::new(zuse_doc::SessionDoc::init("resume").unwrap());
        let (tx, rx) = watch::channel(full.clone());
        let mut stream = doc_message_envelopes(
            rx,
            doc.clone(),
            Some(
                Window::new(64, 128 * 1024)
                    .with_anchor(Some("message-36".into()))
                    .with_resume(Some(hint)),
            ),
        );
        let initial = stream.next().await.unwrap();
        assert!(initial.history_resumed);
        assert_eq!(serde_json::to_value(&initial).unwrap()["historyResumed"],true);
        let zuse_doc::TranscriptFrame::Delta { ref upsert, .. } = initial.update.frame else {
            panic!("page resent cached suffix")
        };
        assert_eq!(upsert.len(), 32);
        assert!(
            upsert
                .iter()
                .all(|row| row.entry.id.parse::<usize>().is_err()
                    && !cached.iter().any(|entry| entry.id == row.entry.id))
        );
        zuse_doc::apply_transcript_frame(&mut cached, initial.update.frame).unwrap();
        assert_eq!(cached, full.entries[36..]);
        let mut changed = full.clone();
        if let zuse_doc::MessagePart::Text { text, .. } =
            &mut Arc::make_mut(&mut changed.entries)[99].parts[0]
        {
            text.push_str(" new live text");
        }
        tx.send_replace(changed.clone());
        let live = stream.next().await.unwrap();
        zuse_doc::apply_transcript_frame(&mut cached, live.update.frame).unwrap();
        assert_eq!(cached, changed.entries[36..]);
        drop(stream);
        assert!(tx.is_closed());
        let stale = zuse_model::TranscriptResume::capture(&full.entries[36..]).unwrap();
        let (_tx, rx) = watch::channel(changed);
        let mut stream = doc_message_envelopes(
            rx,
            doc,
            Some(
                Window::new(64, 128 * 1024)
                    .with_anchor(Some("message-36".into()))
                    .with_resume(Some(stale)),
            ),
        );
        let authoritative = stream.next().await.unwrap();
        assert!(!authoritative.history_resumed);
        assert!(matches!(
            authoritative.update.frame,
            zuse_doc::TranscriptFrame::Reset { .. }
        ));
    }
}
