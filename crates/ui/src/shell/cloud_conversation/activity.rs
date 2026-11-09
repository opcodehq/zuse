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
pub(super) fn command(
    rows: &Value,
    messages: &[zuse_model::SessionMessageEntry],
) -> Option<Activity> {
    let rows = rows.as_array()?;
    // A held queue is not inference. These are execution records, not queue items.
    let eligible = |row: &&Value| {
        row["messageId"].as_str().is_some_and(|id| {
            !id.is_empty()
                && (matches!(
                    row["executionState"].as_str(),
                    Some("running" | "waiting-input" | "outcome-unknown")
                ) || !zuse_viewport::cloud_response_started(messages, id))
        }) && row["text"].is_string()
    };
    let active = |row: &&Value| {
        matches!(
            row["executionState"].as_str(),
            Some("starting" | "running" | "waiting-input" | "outcome-unknown")
        )
    };
    let row = rows
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
        })?;
    let (label, busy) = match row["executionState"].as_str()? {
        "starting" => ("Starting agent turn…", true),
        "running" => ("Agent working…", true),
        "waiting-input" => ("Agent needs your input", false),
        "outcome-unknown" => ("Checking message outcome — review before retrying", false),
        _ => ("Waiting for agent…", true),
    };
    Some(Activity {
        label: label.into(),
        next: None,
        busy,
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
    fn command(rows: &Value) -> Option<Activity> {
        super::command(rows, &[])
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
        for terminal in ["completed", "failed", "cancelled", "rejected"] {
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
        assert!(
            !command(
                &json!([{"executionState":"outcome-unknown","messageId":"message","text":"test"}])
            )
            .unwrap()
            .busy
        );
    }
}
