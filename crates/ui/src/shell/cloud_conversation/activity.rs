//! Presentation follows durable command/preparation facts, never elapsed guesses.
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Activity {
    pub label: String,
    pub next: Option<&'static str>,
    pub busy: bool,
}
pub(super) fn preparation(row: &Value, initial: bool) -> Option<Activity> {
    let progress = zuse_viewport::cloud_preparation_progress(row, initial);
    progress.visible.then(|| Activity {
        label: progress.title,
        next: Some(progress.next),
        busy: progress.busy,
    })
}
/// The composer's status line. Routine startup progress (starting the
/// computer, syncing the repository, preparing the environment or agent
/// access) never shows there: the environment summary card carries the
/// current phase and the transcript's working line stays neutral. Only what
/// the user must act on or know stays: a failed or blocked stage, a lost
/// connection, a credential, capacity or billing block, or an agent issue.
pub(super) fn status_line(
    computer: Option<&Activity>,
    agent: Option<&Activity>,
) -> Option<(String, bool)> {
    computer
        .filter(|stage| !stage.busy)
        .or(agent.filter(|activity| !activity.busy))
        .map(|activity| (activity.label.clone(), false))
}
pub(super) fn command(
    rows: &Value,
    messages: &[zuse_model::SessionMessageEntry],
) -> Option<Activity> {
    let rows = rows.as_array()?;
    // A held queue is not inference. These are execution records, not queue items.
    // A message with an unknown outcome is not progress either: the composer
    // shows it with Retry and Discard instead.
    // A turn the transcript shows finished is over, whatever an older read
    // of its command row still says (a missed end event, an app restart).
    let eligible = |row: &&Value| {
        row["messageId"].as_str().is_some_and(|id| {
            !id.is_empty()
                && !turn_ended(messages, id, row["text"].as_str())
                && (matches!(
                    row["executionState"].as_str(),
                    Some("running" | "waiting-input")
                ) || !zuse_viewport::cloud_response_started(messages, id))
        }) && row["text"].is_string()
    };
    let active = |row: &&Value| {
        matches!(
            row["executionState"].as_str(),
            Some("starting" | "running" | "waiting-input")
        )
    };
    let Some(row) = rows
        .iter()
        .filter(eligible)
        .filter(active)
        .max_by_key(|row| row["createdAt"].as_i64().unwrap_or_default())
        .or_else(|| {
            rows.iter()
                .filter(eligible)
                .filter(|row| {
                    matches!(
                        row["executionState"].as_str(),
                        Some("admitted" | "accepted" | "queued")
                    )
                })
                .max_by_key(|row| row["createdAt"].as_i64().unwrap_or_default())
        })
    else {
        return refused(rows, messages);
    };
    let (label, busy) = match row["executionState"].as_str()? {
        "starting" => ("Starting agent turn…", true),
        "running" => ("Agent working…", true),
        "waiting-input" => ("Agent needs your input", false),
        _ => ("Waiting for agent…", true),
    };
    Some(Activity {
        label: label.into(),
        next: None,
        busy,
    })
}
/// Whether the agent's turn for `message` has ended in the transcript: an
/// assistant message after it is complete or aborted. A message the computer
/// recorded under its own id is found by its text, as the latest such user
/// message.
pub(super) fn turn_ended(
    messages: &[zuse_model::SessionMessageEntry],
    message: &str,
    text: Option<&str>,
) -> bool {
    use zuse_model::{MessagePart, MessageRole, MessageStatus};
    let text_of = |entry: &zuse_model::SessionMessageEntry| {
        entry
            .parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>()
    };
    let start = messages
        .iter()
        .position(|entry| entry.id == message)
        .or_else(|| {
            let text = text?.trim();
            messages
                .iter()
                .rposition(|entry| entry.role == MessageRole::User && text_of(entry).trim() == text)
        });
    let Some(start) = start else {
        return false;
    };
    messages[start + 1..].iter().any(|entry| {
        entry.role == MessageRole::Assistant
            && matches!(
                entry.status,
                Some(MessageStatus::Complete | MessageStatus::Aborted)
            )
    })
}
/// The newest message failed for a known reason before it ran (the computer
/// refused it): say why until another message is sent. A refusal older than
/// the latest turn in the transcript is stale, whether or not that turn's
/// command row was among the rows read, and is never shown again.
fn refused(rows: &[Value], messages: &[zuse_model::SessionMessageEntry]) -> Option<Activity> {
    let newest = rows
        .iter()
        .filter(|row| row["messageId"].as_str().is_some_and(|id| !id.is_empty()))
        .max_by_key(|row| row["createdAt"].as_i64().unwrap_or_default())?;
    if newest["executionState"] != "failed" {
        return None;
    }
    let refused_at = newest["createdAt"].as_i64().unwrap_or_default();
    if messages
        .iter()
        .any(|entry| entry.created_at > refused_at || newest["messageId"] == entry.id.as_str())
    {
        return None;
    }
    let label = zuse_engine::cloud::task_commands::refusal_message(newest["errorCode"].as_str()?)?;
    Some(Activity {
        label: label.into(),
        next: None,
        busy: false,
    })
}
/// The launch prompt, until the transcript holds a user message of its own.
/// The conversation shows it as that first message meanwhile, whatever the
/// launch command's state, so the chat never goes blank.
pub(super) fn pending_initial_prompt(
    first: Option<&str>,
    rows: &Value,
    messages: &[zuse_model::SessionMessageEntry],
) -> Option<String> {
    if messages
        .iter()
        .any(|entry| entry.role == zuse_model::MessageRole::User)
    {
        return None;
    }
    rows.as_array()
        .into_iter()
        .flatten()
        .find(|row| {
            row["commandId"]
                .as_str()
                .is_some_and(|id| id.starts_with("launch:"))
        })
        .and_then(|row| row["text"].as_str())
        .or(first)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn status_line_hides_routine_startup_phases_and_keeps_issues() {
        let line = |row: Value| status_line(preparation(&row, false).as_ref(), None);
        for routine in [
            json!({"state":"provisioning"}),
            json!({"state":"provisioning","startupPhase":"syncing-repository",
                "preparation":{"repository":{"state":"running"}}}),
            json!({"state":"provisioning","preparation":{
                "repository":{"state":"completed"},"environment":{"state":"running"}}}),
            json!({"state":"provisioning","preparation":{
                "repository":{"state":"completed"},"credentials":{"state":"running"}}}),
            json!({"state":"ready","connectionState":"reconnecting"}),
        ] {
            assert_eq!(line(routine.clone()), None, "{routine}");
        }
        for issue in [
            json!({"state":"failed"}),
            json!({"state":"provisioning","preparation":{"repository":{"state":"failed"}}}),
            json!({"state":"provisioning","statusCode":"credential_preparation_failed"}),
            json!({"state":"ready","connectionState":"blocked"}),
        ] {
            let (text, busy) = line(issue.clone()).expect("issues stay visible");
            assert!(!text.is_empty() && !busy, "{issue}");
        }
        // A finished message that needs checking still shows; a busy agent
        // turn belongs to the working line instead.
        let agent = |busy| Activity {
            label: "Needs checking".into(),
            next: None,
            busy,
        };
        assert_eq!(
            status_line(None, Some(&agent(false))),
            Some(("Needs checking".into(), false))
        );
        assert_eq!(status_line(None, Some(&agent(true))), None);
    }
    fn command(rows: &Value) -> Option<Activity> {
        super::command(rows, &[])
    }
    /// A completed turn shows no working line, also when its command row
    /// still reads running (the end event was missed, or the app restarted
    /// and refreshed history).
    #[test]
    fn a_finished_turn_shows_no_working_line_even_with_a_stale_running_row() {
        let entry = |id: &str, role, text: &str, status| zuse_model::SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![zuse_model::MessagePart::Text {
                id: format!("text:{id}"),
                text: text.into(),
            }],
            created_at: 1,
            device_id: String::new(),
            status,
            continuation_of: None,
            duration_ms: None,
        };
        let rows = json!([{"commandId":"c1","messageId":"ask","text":"do you have access?",
            "executionState":"running","createdAt":1}]);
        let mut messages = vec![
            entry(
                "ask",
                zuse_model::MessageRole::User,
                "do you have access?",
                None,
            ),
            entry(
                "reply",
                zuse_model::MessageRole::Assistant,
                "I'll check",
                Some(zuse_model::MessageStatus::Streaming),
            ),
        ];
        assert_eq!(
            super::command(&rows, &messages).unwrap().label,
            "Agent working…",
            "a streaming turn is still working"
        );
        messages[1].status = Some(zuse_model::MessageStatus::Complete);
        assert!(
            super::command(&rows, &messages).is_none(),
            "completed turn: no working line"
        );
        // The same, with the message recorded under the computer's own id.
        messages[0].id = "runtime-id".into();
        assert!(super::command(&rows, &messages).is_none());
    }
    #[test]
    fn launch_prompt_shows_until_the_transcript_owns_a_user_message() {
        assert_eq!(
            pending_initial_prompt(Some("First prompt"), &json!([]), &[]).as_deref(),
            Some("First prompt")
        );
        let rows =
            json!([{"commandId":"launch:task", "text":"Saved prompt", "executionState":"queued"}]);
        assert_eq!(
            pending_initial_prompt(None, &rows, &[]).as_deref(),
            Some("Saved prompt")
        );
        let messages = vec![
            serde_json::from_value::<zuse_model::SessionMessageEntry>(
                json!({"id":"message","role":"user","parts":[],"createdAt":0,"deviceId":"fixture"}),
            )
            .expect("valid user message"),
        ];
        assert!(pending_initial_prompt(Some("First prompt"), &rows, &messages).is_none());
        // A launch past its queue (running, waiting) still has no transcript
        // message of its own; the chat must not go blank meanwhile.
        let running =
            json!([{"commandId":"launch:task", "text":"Saved prompt", "executionState":"running"}]);
        assert_eq!(
            pending_initial_prompt(None, &running, &[]).as_deref(),
            Some("Saved prompt")
        );
        assert!(pending_initial_prompt(None, &json!([{"commandId":"follow-up", "text":"Next prompt", "executionState":"queued"}]), &[]).is_none());
    }
    #[test]
    fn preparation_and_command_activity_use_actual_stages_and_terminal_states() {
        assert_eq!(
            preparation(&json!({"state":"allocating"}), true)
                .unwrap()
                .label,
            "Starting computer"
        );
        assert_eq!(
            preparation(
                &json!({"preparation":{"repository":{"state":"running"}}}),
                true
            )
            .unwrap()
            .next,
            Some("Next: environment setup")
        );
        assert!(
            !preparation(
                &json!({"preparation":{"environment":{"state":"failed"}}}),
                true
            )
            .unwrap()
            .busy
        );
        assert!(preparation(&json!({"state":"paused"}), true).is_none());
        for terminal in [
            "completed",
            "failed",
            "cancelled",
            "rejected",
            "outcome-unknown",
        ] {
            assert!(
                command(&json!([{"executionState":terminal,"messageId":"message","text":"test"}]))
                    .is_none()
            );
        }
        assert_eq!(
            command(&json!([{"executionState":"queued","messageId":"message","text":"test"}]))
                .unwrap()
                .label,
            "Waiting for agent…"
        );
        assert_eq!(
            command(&json!([{"executionState":"running","messageId":"message","text":"test"}]))
                .unwrap()
                .label,
            "Agent working…"
        );
        assert!(
            command(&json!([{"executionState":"running","messageId":null}])).is_none(),
            "control rows are not inference"
        );
        assert_eq!(command(&json!([{"executionState":"queued","messageId":"q","text":"queued","createdAt":2},{"executionState":"running","messageId":"r","text":"run","createdAt":1}])).unwrap().label,"Agent working…");
    }
    #[test]
    fn a_refused_newest_message_says_why_until_another_is_sent() {
        let refused = json!({"executionState":"failed","errorCode":"cloud_command_upload_outside_uploads","messageId":"refused","text":"see file","createdAt":2});
        let shown = command(&json!([refused])).unwrap();
        assert!(shown.label.contains("Attach it again"));
        assert!(!shown.label.contains("didn't run"), "calm, not alarming");
        assert!(!shown.busy);
        // A failure with no known refusal reason (a turn that ran and failed)
        // is not described as a refusal.
        assert!(
            command(&json!([{"executionState":"failed","errorCode":"agent_turn_failed","messageId":"m","text":"t"}]))
                .is_none()
        );
        // A later message replaces the explanation, whether waiting or done.
        let later =
            json!({"executionState":"completed","messageId":"later","text":"again","createdAt":3});
        assert!(command(&json!([refused, later])).is_none());
        let waiting =
            json!({"executionState":"queued","messageId":"later","text":"again","createdAt":3});
        assert_eq!(
            command(&json!([refused, waiting])).unwrap().label,
            "Waiting for agent…"
        );
    }
    fn entry(id: &str, role: &str, created_at: i64) -> zuse_model::SessionMessageEntry {
        serde_json::from_value(json!({"id":id,"role":role,"parts":[{"kind":"text","id":format!("text:{id}"),"text":"done"}],
            "createdAt":created_at,"deviceId":"fixture"}))
        .expect("valid message")
    }
    #[test]
    fn a_refusal_older_than_the_latest_turn_in_the_transcript_is_not_shown() {
        let refused = json!([{"executionState":"failed","errorCode":"cloud_command_upload_missing","messageId":"refused","text":"see file","createdAt":2}]);
        // The transcript only holds turns from before the refusal: it shows.
        let before = vec![entry("old", "user", 1), entry("reply", "assistant", 1)];
        assert!(super::command(&refused, &before).is_some());
        // A later turn ran (its command row may not be among the rows read):
        // the refusal is stale and never shown again.
        let after = vec![entry("old", "user", 1), entry("later", "user", 3)];
        assert!(super::command(&refused, &after).is_none());
        let answered = vec![entry("old", "user", 1), entry("reply", "assistant", 5)];
        assert!(super::command(&refused, &answered).is_none());
        // The refused message itself reaching the transcript means it ran.
        let itself = vec![entry("refused", "user", 1)];
        assert!(super::command(&refused, &itself).is_none());
    }
}
