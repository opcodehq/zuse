//! The note Claude Code adds after every image the model reads, so it can map
//! coordinates back to the full-size file. It rides a user frame but is the
//! CLI talking to the model, never a person: older subagent transcripts
//! stored it as a user message, and a pane full of these read as one prompt
//! sent over and over.

/// Whether `text` is exactly one such note. Matches every spelling the CLI
/// ships (2.1.314): `[Image source: <path>]`, `[Image: source: <path>]`,
/// `[Image: original WxH, displayed at WxH. Multiply coordinates by N to map
/// to original image.]`, and the last two joined by `, `.
pub fn is_image_note(text: &str) -> bool {
    let text = text.trim();
    if text.contains('\n') || !text.ends_with(']') {
        return false;
    }
    if text.starts_with("[Image source: ") {
        return true;
    }
    let Some(body) = text.strip_prefix("[Image: ") else {
        return false;
    };
    body.starts_with("source: ")
        || (body.starts_with("original ")
            && body.contains(", displayed at ")
            && body.ends_with(" to map to original image.]"))
}

#[cfg(test)]
mod tests {
    use super::is_image_note;

    #[test]
    fn matches_every_spelling_the_cli_ships() {
        for note in [
            "[Image source: /tmp/a.png]",
            "[Image: source: /tmp/a.png]",
            "[Image: original 1290x2796, displayed at 923x2000. Multiply coordinates by 1.40 to map to original image.]",
            "[Image: source: /tmp/a.png, original 1576x2152, displayed at 1465x2000. Multiply coordinates by 1.08 to map to original image.]",
            "  [Image: source: /tmp/a.png]\n",
        ] {
            assert!(is_image_note(note), "{note}");
        }
    }

    #[test]
    fn leaves_real_messages_alone() {
        for text in [
            "Look at this [Image: source: /tmp/a.png]",
            "[Image: source: /tmp/a.png]\n\nand fix the header",
            "[Image: the logo]",
            "[Image: original logo please]",
            "Image source: /tmp/a.png",
            "",
        ] {
            assert!(!is_image_note(text), "{text}");
        }
    }
}
