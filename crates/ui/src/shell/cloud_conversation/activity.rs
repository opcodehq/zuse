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
/// Once execution owns the turn, the ordinary transcript owns its working
/// indicator. Keep the composer tray for preparation, queued prompts and issues.
pub(super) fn show_in_tray(row: &Value, activity: &Activity, pending: bool) -> bool {
    pending || activity.next.is_some() || !activity.busy || row["state"] != "ready"
}

/// A saved initial prompt waits in the normal composer tray until the
/// transcript owns its user message. Never render that message twice.
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
                && matches!(
                    row["executionState"].as_str(),
                    Some("admitted" | "accepted" | "queued" | "starting")
                )
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
    fn ready_turn_uses_normal_chat_activity_and_preparation_uses_one_tray() {
        let running = Activity {
            label: "Agent working…".into(),
            next: None,
            busy: true,
        };
        assert!(!show_in_tray(&json!({"state":"ready"}), &running, false));
        assert!(show_in_tray(
            &json!({"state":"allocating"}),
            &running,
            false
        ));
        assert!(show_in_tray(&json!({"state":"ready"}), &running, true));
        let issue = Activity {
            label: "Check message outcome".into(),
            next: None,
            busy: false,
        };
        assert!(show_in_tray(&json!({"state":"ready"}), &issue, false));
    }
    #[test]
    fn initial_prompt_waits_in_tray_only_until_transcript_owns_it() {
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
