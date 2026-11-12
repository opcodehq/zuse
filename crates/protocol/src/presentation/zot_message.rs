//! Display attribution rides the same durable prompt envelope as chat-to-chat
//! messages. It is descriptive context only, never an authorization credential.
use crate::zots::{Zot, ZotAvatar};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZotSender {
    pub zot_id: String,
    pub chat_id: String,
    pub name: String,
    pub avatar: Option<ZotAvatar>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub activity: bool,
}
pub const EXECUTION_CONTEXT: &str = "\n\n[Zot execution context]\n";

pub fn envelope(zot: &Zot, body: &str) -> String {
    attributed(zot, &zot.config.name, body, false)
}

/// A model-free activity notice, rather than a message composed by the coordinator.
pub fn activity(zot: &Zot, body: &str) -> String {
    activity_from(zot, &zot.config.name, body)
}

/// An activity notice displayed as sent by its source, such as a worker thread.
pub fn activity_from(zot: &Zot, source: &str, body: &str) -> String {
    let source = source.trim();
    attributed(
        zot,
        if source.is_empty() {
            &zot.config.name
        } else {
            source
        },
        body,
        true,
    )
}

fn attributed(zot: &Zot, name: &str, body: &str, activity: bool) -> String {
    let sender = ZotSender {
        zot_id: zot.id.clone(),
        chat_id: zot.coordinator_chat_id.clone(),
        name: name.to_owned(),
        avatar: zot.config.avatar.clone(),
        activity,
    };
    format!(
        "[Message from Zot {}]\n\n{body}",
        serde_json::to_string(&sender).expect("sender serializes")
    )
}
/// UI projection also recognizes the existing chat-to-chat routing envelope.
/// Keep it intact for model execution and reply routing.
pub fn parse_display(text: &str) -> Option<(ZotSender, &str)> {
    if let Some(rest) = text.strip_prefix("[Message from Zuse chat ") {
        let (header, body) = rest.split_once("]\n\n")?;
        let (label, destination) =
            header.rsplit_once(". Reply to it with the Zuse `send_message` tool, chat ")?;
        let chat_id = destination.strip_suffix('.')?;
        if chat_id.is_empty() {
            return None;
        }
        let suffix = format!(" ({chat_id})");
        let name = label.strip_suffix(&suffix).unwrap_or(label);
        if name.trim().is_empty() {
            return None;
        }
        return Some((
            ZotSender {
                zot_id: format!("chat:{chat_id}"),
                chat_id: chat_id.to_owned(),
                name: name.to_owned(),
                avatar: None,
                activity: false,
            },
            body,
        ));
    }
    parse(text)
}

pub fn parse(text: &str) -> Option<(ZotSender, &str)> {
    let rest = text.strip_prefix("[Message from Zot ")?;
    let (json, body) = rest.split_once("]\n\n")?;
    let sender: ZotSender = serde_json::from_str(json).ok()?;
    if sender.name.trim().is_empty() || sender.zot_id.is_empty() || sender.chat_id.is_empty() {
        return None;
    }
    Some((
        sender,
        body.split_once(EXECUTION_CONTEXT)
            .map(|(visible, _)| visible)
            .unwrap_or(body),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chat_replies_use_the_same_clean_attribution_without_routing_instructions() {
        let text = "[Message from Zuse chat Build worker (child-42). Reply to it with the Zuse `send_message` tool, chat child-42.]\n\nYour delegated work has finished.\nReview the results.";
        let (sender, body) = parse_display(text).unwrap();
        assert!(
            parse(text).is_none(),
            "chat routing stays intact for execution"
        );
        assert_eq!(sender.name, "Build worker");
        assert_eq!(sender.chat_id, "child-42");
        assert!(!sender.activity);
        assert_eq!(
            body,
            "Your delegated work has finished.\nReview the results."
        );
        assert!(parse_display("[Message from Zuse chat malformed]\n\nBody").is_none());
        assert!(parse_display("An ordinary message with [Message from Zuse chat text").is_none());
    }
    #[test]
    fn attribution_round_trips_without_eating_message_content() {
        let sender = ZotSender {
            zot_id: "z".into(),
            chat_id: "chat".into(),
            name: "A ]\n\n B".into(),
            avatar: None,
            activity: false,
        };
        let input = format!(
            "[Message from Zot {}]\n\nBody\n\nReply",
            serde_json::to_string(&sender).unwrap()
        );
        let (actual, body) = parse(&input).unwrap();
        assert_eq!(actual, sender);
        assert_eq!(body, "Body\n\nReply");
        assert!(parse("[Message from Zot nope]\n\nmessage").is_none());
        assert!(parse("ordinary user message").is_none());
        let enriched = format!("{input}{EXECUTION_CONTEXT}Private engine instructions");
        assert_eq!(parse(&enriched).unwrap().1, "Body\n\nReply");
    }
}
