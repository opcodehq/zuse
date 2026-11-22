//! What the transcript shows for a coordinator turn the engine assembled.
//! The agent still receives the whole prompt (brief, tools, state and the
//! message); the doc entry gets a [`MessageSource::Zapper`] part naming the
//! Zapper and carrying the user's own words or a one-line update summary.
use super::*;
use zuse_protocol::presentation::zapper_message::{self, ZapperSender};
use zuse_protocol::slack::MessageSource;

/// Summaries kept for event wakes still waiting to dispatch. Older rows only
/// matter for a retried dispatch, which falls back to a generic summary.
const KEPT_SUMMARIES: i64 = 500;

impl Zappers {
    /// Remember the short summary for an event wake queued under `message_id`.
    pub(crate) fn record_turn_summary(&self, message_id: &str, summary: &str) -> Result<()> {
        let db = self
            .inner
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?;
        db.execute(
            "INSERT OR REPLACE INTO zapper_turn_summaries(id, summary) VALUES(?1,?2)",
            params![message_id, summary],
        )?;
        db.execute(
            "DELETE FROM zapper_turn_summaries WHERE rowid <= (SELECT MAX(rowid) FROM zapper_turn_summaries) - ?1",
            [KEPT_SUMMARIES],
        )?;
        Ok(())
    }

    fn turn_summary_for(&self, message_id: &str) -> Result<Option<String>> {
        Ok(self
            .inner
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Zapper storage unavailable"))?
            .query_row(
                "SELECT summary FROM zapper_turn_summaries WHERE id=?1",
                [message_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The source to attach to a coordinator turn's doc entry, or `None` for
    /// any other chat. `original` is the prompt as sent, before
    /// [`Zappers::prepare_request`] wrapped it in the brief.
    pub fn turn_source(
        &self,
        chat: &str,
        original: &str,
        message_id: Option<&str>,
    ) -> Result<Option<MessageSource>> {
        let Some(zapper) = self.coordinator(chat)? else {
            return Ok(None);
        };
        // Only an unattributed prompt is the owner's own message; activity
        // wakes and thread reports arrive attributed to their sender.
        let (text, typed) = match zapper_message::parse(original) {
            Some((sender, body)) if sender.activity => {
                let text = match message_id.map(|id| self.turn_summary_for(id)).transpose()? {
                    Some(Some(summary)) => summary,
                    _ => activity_fallback(&sender, body),
                };
                (text, false)
            }
            Some((_, body)) => (body.trim().to_owned(), false),
            None => (original.trim().to_owned(), true),
        };
        Ok(Some(MessageSource::Zapper {
            text,
            typed,
            sender: ZapperSender {
                zapper_id: zapper.id.clone(),
                chat_id: zapper.coordinator_chat_id.clone(),
                name: zapper.config.name.clone(),
                avatar: zapper.config.avatar.clone(),
                activity: false,
            },
        }))
    }
}

/// A summary for an activity wake whose recorded summary is gone.
fn activity_fallback(sender: &ZapperSender, body: &str) -> String {
    let heading = body.lines().next().unwrap_or_default();
    match heading
        .strip_prefix("Activity update: ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse::<usize>().ok())
    {
        Some(n) => format!("{n} updates"),
        None => format!("Update from {}", sender.name),
    }
}

fn counted(n: usize, one: &str, many: &str) -> Option<String> {
    match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    }
}

/// The issue code an `issueChanged` summary starts with ("ZAP-7: title…").
fn issue_code(summary: &str) -> Option<&str> {
    let (code, _) = summary.lines().next()?.split_once(':')?;
    let code = code.trim();
    (!code.is_empty() && code.len() <= 16 && !code.contains(char::is_whitespace)).then_some(code)
}

/// One line for the updates a wake carries, such as
/// "2 thread reports · ZAP-7 changed".
pub(crate) fn turn_summary(events: &[ZapperEvent]) -> String {
    let count = |kind: &str| events.iter().filter(|e| e.kind == kind).count();
    let mut codes: Vec<&str> = Vec::new();
    let mut unnamed_issues = 0;
    for event in events.iter().filter(|e| e.kind == "issueChanged") {
        match issue_code(&event.summary) {
            Some(code) if !codes.contains(&code) => codes.push(code),
            Some(_) => {}
            None => unnamed_issues += 1,
        }
    }
    let known = [
        "settled",
        "failed",
        "needsInput",
        "interrupted",
        "issueChanged",
    ];
    let other = events
        .iter()
        .filter(|e| !known.contains(&e.kind.as_str()))
        .count();
    let issues = if codes.len() <= 3 && unnamed_issues == 0 {
        (!codes.is_empty()).then(|| format!("{} changed", codes.join(", ")))
    } else {
        counted(
            codes.len() + unnamed_issues,
            "issue changed",
            "issues changed",
        )
    };
    let parts: Vec<String> = [
        counted(count("settled"), "thread report", "thread reports"),
        counted(count("failed"), "thread failed", "threads failed"),
        counted(
            count("needsInput"),
            "thread needs input",
            "threads need input",
        ),
        counted(
            count("interrupted"),
            "thread interrupted",
            "threads interrupted",
        ),
        issues,
        counted(other, "other update", "other updates"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        "No updates".into()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zappers::tests::{create, worker};

    fn event(kind: &str, summary: &str) -> ZapperEvent {
        ZapperEvent {
            id: format!("{kind}-{summary}"),
            zapper_id: "release".into(),
            worker_chat_id: "worker".into(),
            kind: kind.into(),
            summary: summary.into(),
            acknowledged: false,
        }
    }

    #[test]
    fn summaries_count_reports_and_name_changed_issues() {
        assert_eq!(
            turn_summary(&[
                event("settled", "Tests pass"),
                event("settled", "PR opened"),
                event("issueChanged", "ZAP-7: Panel tabs\nOwner replied"),
            ]),
            "2 thread reports · ZAP-7 changed"
        );
        assert_eq!(
            turn_summary(&[
                event("failed", "boom"),
                event("needsInput", "which?"),
                event("needsInput", "where?"),
                event("deliveryPending", "offline"),
            ]),
            "1 thread failed · 2 threads need input · 1 other update"
        );
        // The same issue twice is named once; many issues are counted.
        assert_eq!(
            turn_summary(&[
                event("issueChanged", "ZAP-7: a"),
                event("issueChanged", "ZAP-7: b"),
                event("issueChanged", "ZAP-8: c"),
            ]),
            "ZAP-7, ZAP-8 changed"
        );
        assert_eq!(
            turn_summary(&[
                event("issueChanged", "ZAP-1: a"),
                event("issueChanged", "ZAP-2: a"),
                event("issueChanged", "ZAP-3: a"),
                event("issueChanged", "ZAP-4: a"),
            ]),
            "4 issues changed"
        );
        assert_eq!(turn_summary(&[]), "No updates");
    }

    #[test]
    fn coordinator_turns_are_tagged_with_the_user_text_or_a_summary() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let zapper = store.get("release").unwrap();
        let chat = zapper.coordinator_chat_id.clone();

        // A user's own message: the summary is exactly what they typed.
        let source = store
            .turn_source(&chat, "  Ship the release notes  ", Some("m1"))
            .unwrap()
            .unwrap();
        let MessageSource::Zapper {
            text,
            sender,
            typed,
        } = &source
        else {
            panic!("expected a Zapper source");
        };
        assert_eq!(text, "Ship the release notes");
        assert!(typed, "the owner's message shows as their own");
        assert_eq!(sender.zapper_id, "release");
        assert_eq!(sender.chat_id, chat);
        assert_eq!(sender.name, zapper.config.name);
        assert!(!sender.activity);

        // An event wake: the summary recorded when it was queued.
        let notice =
            zapper_message::activity_from(&zapper, "Thread", "Activity update: 2 updates\n…");
        store
            .record_turn_summary("wake-1", "2 thread reports")
            .unwrap();
        assert_eq!(
            store
                .turn_source(&chat, &notice, Some("wake-1"))
                .unwrap()
                .unwrap()
                .text(),
            "2 thread reports"
        );
        assert!(
            !store
                .turn_source(&chat, &notice, Some("wake-1"))
                .unwrap()
                .unwrap()
                .is_typed(),
            "engine updates keep the Zapper header"
        );
        // Without the record it still never shows the raw updates.
        assert_eq!(
            store
                .turn_source(&chat, &notice, Some("lost"))
                .unwrap()
                .unwrap()
                .text(),
            "2 updates"
        );
        let single =
            zapper_message::activity_from(&zapper, "Build thread", "Activity update\nDone");
        assert_eq!(
            store
                .turn_source(&chat, &single, None)
                .unwrap()
                .unwrap()
                .text(),
            "Update from Build thread"
        );

        // Threads and ordinary chats are left alone.
        store
            .command(crate::zappers::tests::command(
                "release",
                "adopt",
                ZapperAction::AdoptWorker { worker: worker() },
            ))
            .unwrap();
        assert!(store.turn_source("worker", "Hi", None).unwrap().is_none());
        assert!(store.turn_source("plain", "Hi", None).unwrap().is_none());
    }

    #[test]
    fn the_prompt_sent_to_the_coordinator_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        let chat = store.get("release").unwrap().coordinator_chat_id;
        let mut request: zuse_protocol::RunRequest = serde_json::from_value(
            serde_json::json!({"prompt": "Ship it", "cwd": "/tmp", "sandbox": "workspace-write"}),
        )
        .unwrap();
        store
            .prepare_request(&chat, zuse_protocol::HarnessId::Codex, &mut request)
            .unwrap();
        assert!(request.prompt.ends_with("## User message\nShip it"));
        assert!(request.prompt.contains("## Hard rules"));
        // The label stays the user's own text, not the assembled turn.
        assert_eq!(
            store
                .turn_source(&chat, "Ship it", None)
                .unwrap()
                .unwrap()
                .text(),
            "Ship it"
        );
    }
}
