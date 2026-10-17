//! Queue controls use the host's delivery barrier and explicit RPC outcomes.
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueAction {
    Move,
    Remove,
    Steer,
    SendNow,
}
impl QueueAction {
    pub fn method(self) -> &'static str {
        match self {
            Self::Move => crate::methods::MOVE_QUEUED_MESSAGE,
            Self::Remove => crate::methods::REMOVE_QUEUED_MESSAGE,
            Self::Steer => crate::methods::STEER_QUEUED_MESSAGE_NOW,
            Self::SendNow => crate::methods::SEND_QUEUED_MESSAGE_NOW,
        }
    }
    pub fn confirmed(self, reply: &Value) -> Option<bool> {
        reply[match self {
            Self::Move => "changed",
            Self::Remove => "removed",
            Self::Steer | Self::SendNow => "sent",
        }]
        .as_bool()
    }
}

/// Bind all queue/transcript/command operations to the chat's execution host.
/// A reconnect must use the same scope as the original subscription/action.
pub fn chat_request(chat: &Value, params: Value) -> Value {
    let mut params = params.as_object().cloned().unwrap_or_else(Map::new);
    params.insert("chatId".into(), chat["id"].clone());
    params.insert("targetDeviceId".into(), chat["deviceId"].clone());
    Value::Object(params)
}

pub struct QueueRowDetails {
    pub title: String,
    pub attachment_count: usize,
    pub blocked: Option<&'static str>,
    pub valid_id: bool,
}
impl QueueRowDetails {
    pub fn allows(&self, action: QueueAction) -> bool {
        self.valid_id
            && match action {
                QueueAction::Move | QueueAction::Remove => true,
                QueueAction::SendNow => self.blocked.is_none(),
                QueueAction::Steer => self.blocked.is_none() && self.attachment_count == 0,
            }
    }
}
pub fn queue_row_details(row: &Value) -> QueueRowDetails {
    let parsed = crate::message_attachments::parse(row["text"].as_str().unwrap_or_default());
    let mut files = std::collections::BTreeSet::new();
    for attachment in &parsed.attachments {
        files.insert(attachment.path.as_str());
    }
    let invalid_attachments = !row["attachments"].is_null()
        && (!row["attachments"].is_array()
            || row["attachments"]
                .as_array()
                .is_some_and(|files| files.iter().any(|file| !file.is_string())));
    for file in row["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        files.insert(file);
    }
    let attachment_count = files.len();
    let title = parsed.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let blocked = if invalid_attachments {
        Some("Attachment data unavailable")
    } else if row["deliveryGate"].is_null() {
        None
    } else {
        Some(match row["deliveryGate"]["kind"].as_str() {
            Some("editing") => "Being edited",
            Some("reviewRequired") => "Review required",
            _ => "Waiting for host review",
        })
    };
    QueueRowDetails {
        title: if title.is_empty() {
            if attachment_count > 0 {
                format!(
                    "({attachment_count} file{})",
                    if attachment_count == 1 { "" } else { "s" }
                )
            } else {
                "(empty)".into()
            }
        } else {
            title
        },
        attachment_count,
        blocked,
        valid_id: row["id"].as_str().is_some_and(|id| !id.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn chat_scope_keeps_remote_host_and_payload_even_with_conflicting_fields() {
        let chat = json!({"id":"thread","deviceId":"linux-host"});
        let params = chat_request(
            &chat,
            json!({"chatId":"wrong","targetDeviceId":"mac-viewer","id":"queued","attachments":["/host/image.png"],"text":"keep this","toIndex":2}),
        );
        assert_eq!(
            params,
            json!({"chatId":"thread","targetDeviceId":"linux-host","id":"queued","attachments":["/host/image.png"],"text":"keep this","toIndex":2})
        );
        assert!(chat_request(&json!({"id":"legacy"}), json!({}))["targetDeviceId"].is_null());
    }
    #[test]
    fn edit_review_and_unknown_barriers_never_enable_delivery_in_the_view() {
        for gate in [
            json!({"kind":"editing","expiresAtMs":0}),
            json!({"kind":"reviewRequired"}),
            json!({"kind":"futureBarrier"}),
            json!(false),
        ] {
            let row = queue_row_details(&json!({"id":"row","text":"hello","deliveryGate":gate}));
            assert!(row.blocked.is_some());
            assert!(!row.allows(QueueAction::Steer));
            assert!(!row.allows(QueueAction::SendNow));
            assert!(row.allows(QueueAction::Move));
            assert!(row.allows(QueueAction::Remove));
        }
    }
    #[test]
    fn attachment_rows_allow_explicit_send_but_never_mid_turn_steering() {
        let text = crate::message_attachments::with_attachments(
            "hello\n there",
            &["/host/image.png".into()],
        );
        let row =
            queue_row_details(&json!({"id":"row","text":text,"attachments":["/host/image.png"]}));
        assert_eq!(row.title, "hello there");
        assert_eq!(row.attachment_count, 1);
        assert!(!row.allows(QueueAction::Steer));
        assert!(row.allows(QueueAction::SendNow));
        let invalid = queue_row_details(&json!({"id":"row","text":"hello","attachments":[null]}));
        assert!(!invalid.allows(QueueAction::Steer));
        assert!(!invalid.allows(QueueAction::SendNow));
        assert!(!queue_row_details(&json!({"text":"hello"})).allows(QueueAction::Remove));
    }
    #[test]
    fn acknowledgements_are_strict_and_method_specific() {
        for (action, field) in [
            (QueueAction::Move, "changed"),
            (QueueAction::Remove, "removed"),
            (QueueAction::Steer, "sent"),
            (QueueAction::SendNow, "sent"),
        ] {
            assert_eq!(action.confirmed(&json!({field:true})), Some(true));
            assert_eq!(action.confirmed(&json!({field:false})), Some(false));
            assert_eq!(action.confirmed(&json!({field:"true"})), None);
            assert_eq!(action.confirmed(&json!({"ok":true})), None);
        }
    }
}
