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

/// The single trailing action a queue row advertises and executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePrimaryAction {
    Steer,
    SendNext,
    SendNow,
}

impl QueuePrimaryAction {
    pub fn action(self) -> QueueAction {
        match self {
            Self::Steer | Self::SendNext => QueueAction::Steer,
            Self::SendNow => QueueAction::SendNow,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Steer => "Steer",
            Self::SendNext => "Send next",
            Self::SendNow => "Send now",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Steer => "Steer (keep current work running)",
            Self::SendNext => "Send at the next turn (this agent cannot steer mid-turn)",
            Self::SendNow => "Send now (interrupt)",
        }
    }
}

/// Text uses the live mailbox, including providers that consume at turn boundaries.
/// Attachments still require a new request and advertise the interruption.
pub fn queue_primary_action(
    delivery_blocked: bool,
    host_supports_actions: bool,
    has_attachments: bool,
    steers_mid_turn: bool,
) -> Option<QueuePrimaryAction> {
    (!delivery_blocked && host_supports_actions).then_some(if has_attachments {
        QueuePrimaryAction::SendNow
    } else if steers_mid_turn {
        QueuePrimaryAction::Steer
    } else {
        QueuePrimaryAction::SendNext
    })
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

/// The host's acquisition receipt is the only authority to replace a draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEditLease {
    pub id: String,
    pub text: String,
    pub attachments: Vec<String>,
    pub base_text_hash: String,
}
pub fn queue_edit_lease(reply: &Value) -> Option<QueueEditLease> {
    if reply["outcome"] != "acquired" || reply["expiresAtMs"].as_i64()? <= 0 {
        return None;
    }
    let id = reply["leaseId"]
        .as_str()
        .filter(|value| !value.is_empty())?;
    let hash = reply["baseTextHash"]
        .as_str()
        .filter(|value| !value.is_empty())?;
    let attachments = reply["attachments"]
        .as_array()?
        .iter()
        .map(|path| {
            path.as_str()
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(QueueEditLease {
        id: id.into(),
        text: reply["text"].as_str()?.into(),
        attachments,
        base_text_hash: hash.into(),
    })
}
pub fn queue_edit_review_matches(row: &Value, lease: &str) -> bool {
    !lease.is_empty()
        && row["deliveryGate"]["kind"] == "reviewRequired"
        && row["deliveryGate"]["previousLeaseId"].as_str() == Some(lease)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn primary_delivery_modes_keep_text_non_interrupting_and_files_explicit() {
        let steer = queue_primary_action(false, true, false, true).unwrap();
        let next = queue_primary_action(false, true, false, false).unwrap();
        let files = queue_primary_action(false, true, true, true).unwrap();
        assert_eq!(
            (steer.label(), steer.action()),
            ("Steer", QueueAction::Steer)
        );
        assert_eq!(
            (next.label(), next.action()),
            ("Send next", QueueAction::Steer)
        );
        assert_eq!(
            (files.label(), files.action()),
            ("Send now", QueueAction::SendNow)
        );
        assert!(queue_primary_action(true, true, true, true).is_none());
        assert!(queue_primary_action(false, false, false, true).is_none());
    }
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
    #[test]
    fn edit_acquisition_requires_a_complete_host_receipt() {
        let valid = json!({"outcome":"acquired","leaseId":"lease","baseTextHash":"hash","text":"queued text","attachments":["/host/file.txt"],"expiresAtMs":1234});
        let lease = queue_edit_lease(&valid).unwrap();
        assert_eq!(lease.text, "queued text");
        assert_eq!(lease.attachments, vec!["/host/file.txt"]);
        for (field, value) in [
            ("outcome", json!("locked")),
            ("leaseId", json!("")),
            ("baseTextHash", json!(null)),
            ("text", json!(null)),
            ("attachments", json!([null])),
            ("expiresAtMs", json!(0)),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(queue_edit_lease(&invalid).is_none(), "{field}");
        }
    }
    #[test]
    fn expired_edit_review_accepts_only_its_previous_lease_generation() {
        let row = json!({"deliveryGate":{"kind":"reviewRequired","previousLeaseId":"old"}});
        assert!(queue_edit_review_matches(&row, "old"));
        assert!(!queue_edit_review_matches(&row, "new"));
        assert!(!queue_edit_review_matches(
            &json!({"deliveryGate":{"kind":"editing","leaseId":"old"}}),
            "old"
        ));
        assert!(!queue_edit_review_matches(
            &json!({"deliveryGate":{"kind":"reviewRequired","previousLeaseId":""}}),
            ""
        ));
    }
}
