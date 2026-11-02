//! Split persisted attachment references from the user-visible message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageAttachment {
    pub path: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentMessage {
    pub text: String,
    pub attachments: Vec<MessageAttachment>,
}
pub fn parse(content: &str) -> AttachmentMessage {
    let plain = || AttachmentMessage {
        text: content.into(),
        attachments: vec![],
    };
    let lower = content.to_ascii_lowercase();
    let needle = "\n\nattached images (local files";
    let mut from = 0;
    while let Some(relative) = lower[from..].find(needle) {
        let gap = from + relative;
        let line_start = gap + 2;
        let line_end = content[line_start..]
            .find('\n')
            .map_or(content.len(), |i| line_start + i);
        from = line_start;
        if !content[line_start..line_end]
            .trim_end_matches('\r')
            .ends_with("):")
        {
            continue;
        }
        let trailer = &content[(line_end + 1).min(content.len())..];
        let mut attachments = vec![];
        for line in trailer.lines().filter(|line| !line.trim().is_empty()) {
            // Preserve the original message when a purported trailer contains prose.
            let Some(path) = line
                .trim_start()
                .strip_prefix("- ")
                .map(str::trim)
                .filter(|p| !p.is_empty())
            else {
                return plain();
            };
            attachments.push(MessageAttachment {
                name: super::attachment_mentions::attachment_display_name(
                    path.rsplit(['/', '\\']).next().unwrap_or(path),
                )
                .to_owned(),
                path: path.into(),
            });
        }
        if attachments.is_empty() {
            return plain();
        }
        let body = content[..gap].trim_end();
        return AttachmentMessage {
            text: if matches!(
                body,
                "See the attached image(s)." | "See the attached file(s)."
            ) {
                String::new()
            } else {
                body.into()
            },
            attachments,
        };
    }
    plain()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trailers_preserve_space_paths_and_hide_only_transport_text() {
        let parsed = parse(
            "Explain this\n\nAttached images (local files — open them to view):\n- /uploads/a cat.png\n- /uploads/notes.md",
        );
        assert_eq!(parsed.text, "Explain this");
        assert_eq!(parsed.attachments[0].path, "/uploads/a cat.png");
        assert_eq!(parsed.attachments[1].name, "notes.md");
        assert!(
            parse("See the attached image(s).\n\nATTACHED IMAGES (LOCAL FILES):\n- /uploads/a.png")
                .text
                .is_empty()
        );
    }
    #[test]
    fn malformed_or_embedded_markers_preserve_the_original_message() {
        for body in [
            "Hello\n\nAttached images (local files):\nNo attachment",
            "Hello\n\nAttached images (local files):",
            "Attached images (local files):\n- /a.png",
            "Hi\n\nAttached images (local files):\n- /a.png\nKeep this prose",
        ] {
            assert_eq!(parse(body).text, body);
            assert!(parse(body).attachments.is_empty());
        }
    }
}

/// Assemble a bounded attachment read. A malformed cursor never commits bytes.
#[derive(Default)]
pub struct AttachmentBytes(Vec<u8>);
impl AttachmentBytes {
    pub const MAX_BYTES: usize = 8 * 1024 * 1024;
    pub fn offset(&self) -> u64 {
        self.0.len() as u64
    }
    pub fn append(&mut self, chunk: &[u8], next: u64, done: bool) -> Result<(), &'static str> {
        if next != self.offset().saturating_add(chunk.len() as u64) || (!done && chunk.is_empty()) {
            return Err("Image read did not advance consistently.");
        }
        if self.0.len().saturating_add(chunk.len()) > Self::MAX_BYTES {
            return Err("Image is larger than the 8 MB preview limit.");
        }
        self.0.extend_from_slice(chunk);
        Ok(())
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}
#[cfg(test)]
mod read_tests {
    use super::AttachmentBytes;
    #[test]
    fn chunk_reads_reject_stale_or_nonprogressing_cursors_without_committing() {
        let mut read = AttachmentBytes::default();
        read.append(b"ab", 2, false).unwrap();
        assert!(read.append(b"cd", 3, false).is_err());
        assert!(read.append(b"", 2, false).is_err());
        assert_eq!(read.offset(), 2);
        read.append(b"cd", 4, false).unwrap();
        read.append(b"", 4, true).unwrap();
        assert_eq!(read.into_bytes(), b"abcd");
    }
    #[test]
    fn bounded_reads_reject_the_first_byte_over_the_limit() {
        let mut read = AttachmentBytes::default();
        read.append(
            &vec![0; AttachmentBytes::MAX_BYTES],
            AttachmentBytes::MAX_BYTES as u64,
            false,
        )
        .unwrap();
        assert!(
            read.append(b"x", AttachmentBytes::MAX_BYTES as u64 + 1, true)
                .is_err()
        );
        assert_eq!(read.offset(), AttachmentBytes::MAX_BYTES as u64);
    }
}

/// Persist attachment references with the prompt so reopening a chat retains them.
/// Existing matching trailers are preserved rather than duplicated on retry.
pub fn with_attachments(text: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return text.into();
    }
    let trailer = format!(
        "\n\nAttached images (local files — open them to view):\n{}",
        paths
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let body = text.strip_suffix(&trailer).unwrap_or(text);
    let body = if body.trim().is_empty() {
        if paths
            .iter()
            .all(|p| super::attachment_mentions::is_image_path(p))
        {
            "See the attached image(s)."
        } else {
            "See the attached file(s)."
        }
    } else {
        body
    };
    format!("{body}{trailer}")
}
#[cfg(test)]
mod transport_tests {
    use super::*;
    #[test]
    fn transport_roundtrips_image_only_and_file_messages_and_is_retry_idempotent() {
        for (body, paths) in [
            ("", vec!["/uploads/screen.png".into()]),
            ("", vec!["/uploads/notes.md".into()]),
            (
                "Please inspect",
                vec!["/uploads/a cat.png".into(), "/uploads/notes.md".into()],
            ),
        ] {
            let transport = with_attachments(body, &paths);
            assert_eq!(with_attachments(&transport, &paths), transport);
            let parsed = parse(&transport);
            assert_eq!(parsed.text, body);
            assert_eq!(
                parsed
                    .attachments
                    .into_iter()
                    .map(|a| a.path)
                    .collect::<Vec<_>>(),
                paths
            );
        }
    }
}
