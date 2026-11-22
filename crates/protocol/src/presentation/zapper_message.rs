//! Display attribution rides the same durable prompt envelope as chat-to-chat
//! messages. It is descriptive context only, never an authorization credential.
use crate::zappers::{Zapper, ZapperAvatar};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZapperSender {
    // Legacy, remove after one release: transcripts saved before the rename
    // carry `zotId`.
    #[serde(alias = "zotId")]
    pub zapper_id: String,
    pub chat_id: String,
    pub name: String,
    pub avatar: Option<ZapperAvatar>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub activity: bool,
}
/// Opens a Zapper-attributed message. Models read this, so it names the
/// product; [`LEGACY_PREFIX`] still parses in transcripts saved before.
pub const PREFIX: &str = "[Message from Zapper ";
// Legacy, remove after one release.
const LEGACY_PREFIX: &str = "[Message from Zot ";
pub const EXECUTION_CONTEXT: &str = "\n\n[Zapper execution context]\n";
// Legacy, remove after one release.
const LEGACY_EXECUTION_CONTEXT: &str = "\n\n[Zot execution context]\n";

pub fn envelope(zapper: &Zapper, body: &str) -> String {
    attributed(zapper, &zapper.config.name, body, false)
}

/// A model-free activity notice, rather than a message composed by the coordinator.
pub fn activity(zapper: &Zapper, body: &str) -> String {
    activity_from(zapper, &zapper.config.name, body)
}

/// An activity notice displayed as sent by its source, such as a worker thread.
pub fn activity_from(zapper: &Zapper, source: &str, body: &str) -> String {
    let source = source.trim();
    attributed(
        zapper,
        if source.is_empty() {
            &zapper.config.name
        } else {
            source
        },
        body,
        true,
    )
}

fn attributed(zapper: &Zapper, name: &str, body: &str, activity: bool) -> String {
    let sender = ZapperSender {
        zapper_id: zapper.id.clone(),
        chat_id: zapper.coordinator_chat_id.clone(),
        name: name.to_owned(),
        avatar: zapper.config.avatar.clone(),
        activity,
    };
    format!(
        "{PREFIX}{}]\n\n{body}",
        serde_json::to_string(&sender).expect("sender serializes")
    )
}
/// UI projection also recognizes the existing chat-to-chat routing envelope.
/// Keep it intact for model execution and reply routing.
pub fn parse_display(text: &str) -> Option<(ZapperSender, &str)> {
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
            ZapperSender {
                zapper_id: format!("chat:{chat_id}"),
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

/// The message up to the first execution-context marker, current or legacy,
/// so private host context never shows as message text whichever comes first.
fn visible_body(body: &str) -> &str {
    [EXECUTION_CONTEXT, LEGACY_EXECUTION_CONTEXT]
        .iter()
        .filter_map(|marker| body.find(marker))
        .min()
        .map_or(body, |at| &body[..at])
}

pub fn parse(text: &str) -> Option<(ZapperSender, &str)> {
    let rest = text
        .strip_prefix(PREFIX)
        .or_else(|| text.strip_prefix(LEGACY_PREFIX))?;
    let (json, body) = rest.split_once("]\n\n")?;
    let sender: ZapperSender = serde_json::from_str(json).ok()?;
    if sender.name.trim().is_empty() || sender.zapper_id.is_empty() || sender.chat_id.is_empty() {
        return None;
    }
    Some((sender, visible_body(body)))
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
        let sender = ZapperSender {
            zapper_id: "z".into(),
            chat_id: "chat".into(),
            name: "A ]\n\n B".into(),
            avatar: None,
            activity: false,
        };
        let input = format!(
            "[Message from Zapper {}]\n\nBody\n\nReply",
            serde_json::to_string(&sender).unwrap()
        );
        let (actual, body) = parse(&input).unwrap();
        assert_eq!(actual, sender);
        assert_eq!(body, "Body\n\nReply");
        assert!(parse("[Message from Zapper nope]\n\nmessage").is_none());
        assert!(parse("ordinary user message").is_none());
        let enriched = format!("{input}{EXECUTION_CONTEXT}Private engine instructions");
        assert_eq!(parse(&enriched).unwrap().1, "Body\n\nReply");
        let legacy = format!("{input}{LEGACY_EXECUTION_CONTEXT}Private engine instructions");
        assert_eq!(parse(&legacy).unwrap().1, "Body\n\nReply");
        // A legacy marker ahead of a current one still ends the visible text.
        let both =
            format!("{input}{LEGACY_EXECUTION_CONTEXT}Private{EXECUTION_CONTEXT}More private");
        assert_eq!(parse(&both).unwrap().1, "Body\n\nReply");
    }
    #[test]
    fn new_messages_name_the_zapper() {
        let sender = ZapperSender {
            zapper_id: "z".into(),
            chat_id: "chat".into(),
            name: "Release".into(),
            avatar: None,
            activity: false,
        };
        let text = format!(
            "{PREFIX}{}]\n\nShip it",
            serde_json::to_string(&sender).unwrap()
        );
        assert!(text.starts_with("[Message from Zapper "));
        assert!(!text.contains(crate::zappers::legacy::ID_FIELD));
        assert_eq!(parse(&text).unwrap().1, "Ship it");
    }
}
