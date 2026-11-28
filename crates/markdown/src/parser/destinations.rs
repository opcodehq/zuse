//! Preserve workspace paths with spaces in Markdown link destinations.

use std::ops::Range;

/// Byte positions, in the rewritten text, of the angle brackets the rewrite
/// inserted. A rewritten offset maps back to the original by subtracting the
/// insertions before it; nothing is ever removed, so the mapping is exact.
#[derive(Debug, Default)]
pub(super) struct Insertions {
    positions: Vec<usize>,
}

impl Insertions {
    fn original(&self, offset: usize) -> usize {
        offset - self.positions.partition_point(|&at| at < offset)
    }

    pub(super) fn original_range(&self, range: Range<usize>) -> Range<usize> {
        self.original(range.start)..self.original(range.end)
    }
}

/// Wrap inline link and image destinations that spell an absolute path with
/// literal spaces in pulldown-cmark's angle-bracket form: CommonMark ends a
/// destination at its first space, so such a link otherwise stays plain text.
/// Code keeps its spelling — a fenced block, an indented line or an inline
/// code span is copied verbatim — and a destination that is already bracketed
/// or whose closing `)` has not streamed in yet is left alone, which makes
/// the rewrite idempotent and safe while text is still arriving.
pub(super) fn rewrite_absolute_destinations(source: &str) -> Option<(String, Insertions)> {
    if !source.contains("](") {
        return None;
    }
    let bytes = source.as_bytes();
    let mut rewritten: Option<String> = None;
    let mut insertions = Insertions::default();
    // Bytes before this are already in `rewritten`; skipped regions advance
    // it without copying, so the next splice stays contiguous.
    let mut copied = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        if at == 0 || bytes[at - 1] == b'\n' {
            if let Some(fence) = fence_open(&source[at..]) {
                let end = skip_fenced_block(source, at, fence);
                skip(&mut rewritten, source, &mut copied, end);
                at = end;
                continue;
            }
            if line_is_indented_code(&source[at..]) {
                let end = line_end(source, at);
                skip(&mut rewritten, source, &mut copied, end);
                at = end;
                continue;
            }
        }
        match bytes[at] {
            b'`' if !is_escaped(bytes, at) => {
                let run = backtick_run(bytes, at);
                let end = match closing_backtick_run(bytes, at + run, run) {
                    Some(close) => close + run,
                    None => at + run,
                };
                skip(&mut rewritten, source, &mut copied, end);
                at = end;
            }
            b']' if bytes.get(at + 1) == Some(&b'(') && !is_escaped(bytes, at) => {
                let start = skip_destination_spaces(bytes, at + 2);
                let Some(close) = destination_close(bytes, start) else {
                    at += 2;
                    continue;
                };
                let destination = source[start..close].trim_end_matches(' ');
                if absolute_destination_with_spaces(destination) {
                    let out = match &mut rewritten {
                        Some(out) => out,
                        None => rewritten.insert({
                            // Nothing is copied until the first rewrite; the
                            // skipped code regions ahead of it come along with
                            // the prefix.
                            let mut out = String::with_capacity(source.len() + 8);
                            out.push_str(&source[..copied]);
                            out
                        }),
                    };
                    out.push_str(&source[copied..start]);
                    insertions.positions.push(out.len());
                    out.push('<');
                    out.push_str(destination);
                    insertions.positions.push(out.len());
                    out.push('>');
                    copied = start + destination.len();
                }
                at = close + 1;
            }
            _ => at += 1,
        }
    }
    let mut out = rewritten?;
    out.push_str(&source[copied..]);
    Some((out, insertions))
}

/// Advance the copy cursor over a region that keeps its spelling, appending
/// it to the rewritten text when a rewrite is already in progress.
fn skip(rewritten: &mut Option<String>, source: &str, copied: &mut usize, until: usize) {
    if let Some(out) = rewritten {
        out.push_str(&source[*copied..until]);
    }
    *copied = until;
}

/// Whether a destination needs the angle-bracket rewrite: an absolute path
/// with at least one literal space that stays inside the absolute-path
/// envelope the link classifier accepts. `file://` destinations are exempt
/// from the file-name dot rule, exactly as they are when classified.
fn absolute_destination_with_spaces(destination: &str) -> bool {
    if !destination.contains(' ') {
        return false;
    }
    let path = match destination.strip_prefix("file://") {
        Some(rest) => {
            // Only an empty host keeps this a file path; `file://localhost`
            // and friends are ordinary URLs.
            let host_end = rest.find('/').unwrap_or(rest.len());
            if !rest[..host_end].is_empty() {
                return false;
            }
            &rest[host_end..]
        }
        None if destination.starts_with('/') => destination,
        None => return false,
    };
    if path.len() <= 1
        || path.starts_with("//")
        || path.ends_with('/')
        || destination.contains(['\\', '?'])
        || destination.chars().any(char::is_control)
        || destination
            .split('/')
            .any(|part| matches!(part, "." | ".."))
    {
        return false;
    }
    let file_name = path.rsplit('/').next().unwrap_or(path);
    destination.starts_with("file://") || file_name.contains('.')
}

/// A fence opener: up to three leading spaces, then at least three backticks
/// or tildes. Returns the fence byte and its run length.
fn fence_open(line: &str) -> Option<(u8, usize)> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|&&byte| byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let fence = *bytes.get(indent)?;
    if !matches!(fence, b'`' | b'~') {
        return None;
    }
    let len = bytes[indent..].iter().take_while(|&&b| b == fence).count();
    (len >= 3).then_some((fence, len))
}

/// The byte after a fenced block's closing line (or the end of the source for
/// a still-open fence, whose body is literal by definition).
fn skip_fenced_block(source: &str, at: usize, (fence, len): (u8, usize)) -> usize {
    let bytes = source.as_bytes();
    let mut cursor = line_end(source, at);
    while cursor < bytes.len() {
        let end = line_end(source, cursor);
        let line = &bytes[cursor..end];
        let indent = line.iter().take_while(|&&byte| byte == b' ').count();
        if indent <= 3 {
            let run = line[indent..].iter().take_while(|&&b| b == fence).count();
            let rest = line.get(indent + run..).unwrap_or_default();
            if run >= len
                && rest
                    .iter()
                    .all(|&byte| byte == b' ' || byte == b'\n' || byte == b'\r')
            {
                return end;
            }
        }
        cursor = end;
    }
    bytes.len()
}

/// Whether a line opens indented code: four leading spaces or a tab. The
/// rewrite only needs the conservative direction — a line it treats as code
/// just keeps today's plain-text rendering.
fn line_is_indented_code(line: &str) -> bool {
    line.starts_with('\t') || line.as_bytes().iter().take_while(|&&b| b == b' ').count() >= 4
}

fn line_end(source: &str, at: usize) -> usize {
    source.as_bytes()[at..]
        .iter()
        .position(|&byte| byte == b'\n')
        .map(|run| at + run + 1)
        .unwrap_or(source.len())
}

/// An odd number of backslashes before `at` escapes the byte there.
fn is_escaped(bytes: &[u8], at: usize) -> bool {
    bytes[..at]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 1
}

fn backtick_run(bytes: &[u8], at: usize) -> usize {
    bytes[at..].iter().take_while(|&&byte| byte == b'`').count()
}

/// The start of the next backtick run as long as `run`, if any: an inline
/// code span either closes with its own run length or stays literal.
fn closing_backtick_run(bytes: &[u8], from: usize, run: usize) -> Option<usize> {
    let mut at = from;
    while at < bytes.len() {
        if bytes[at] == b'`' {
            let len = backtick_run(bytes, at);
            if len == run {
                return Some(at);
            }
            at += len;
        } else {
            at += 1;
        }
    }
    None
}

fn skip_destination_spaces(bytes: &[u8], at: usize) -> usize {
    let mut at = at;
    while bytes.get(at) == Some(&b' ') {
        at += 1;
    }
    at
}

/// The first unescaped `)` at paren depth zero — the destination's end. `)`
/// inside balanced parentheses belongs to the path, an escaped `)` does not
/// close, and a control character (a newline mid-destination, a tab) leaves
/// the link alone rather than guessing at its shape.
fn destination_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut at = start;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'(' => {
                depth += 1;
                at += 1;
            }
            b')' if depth == 0 => return Some(at),
            b')' => {
                depth -= 1;
                at += 1;
            }
            byte if byte.is_ascii_control() => return None,
            _ => at += 1,
        }
    }
    None
}

#[cfg(test)]
mod destination_rewrite_tests {
    use super::*;
    use crate::parser::{Block, IncrementalParser, InlineRun, TopBlock, parse_full};

    fn paragraph(source: &str) -> Vec<InlineRun> {
        let tree = parse_full(source);
        let Some(TopBlock {
            block: Block::Paragraph { runs },
            ..
        }) = tree.blocks.first().map(|top| top.as_ref())
        else {
            panic!("expected a paragraph for {source:?}");
        };
        runs.clone()
    }

    fn link_of(runs: &[InlineRun]) -> Option<&str> {
        runs.iter().find_map(|run| run.style.link.as_deref())
    }

    fn text(runs: &[InlineRun]) -> String {
        runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn absolute_destinations_with_spaces_become_links() {
        let runs = paragraph("see [it's here](/tmp/2026/Some Folder/it's here.txt) now");
        assert_eq!(text(&runs), "see it's here now");
        assert_eq!(link_of(&runs), Some("/tmp/2026/Some Folder/it's here.txt"));

        let runs = paragraph("[x](file:///tmp/a b/c.md)");
        assert_eq!(link_of(&runs), Some("file:///tmp/a b/c.md"));

        let runs = paragraph("shot ![two](/tmp/pics/two words.png) done");
        let image = runs.iter().find_map(|run| run.style.image.as_ref());
        assert_eq!(
            image.map(|image| image.source.as_str()),
            Some("/tmp/pics/two words.png")
        );
    }

    #[test]
    fn non_absolute_or_unsafe_destinations_keep_their_text() {
        for source in [
            // Relative destinations with spaces stay plain text.
            "[x](Some Folder/a.md)",
            // The file-name dot rule applies to plain absolute paths.
            "[x](/usr/bin/some thing)",
            // `file://localhost` is an ordinary URL, not a host path.
            "[x](file://localhost/tmp/a b.md)",
            "[x](/tmp/a b/c/)",
            "[x](/tmp/a?b c.md)",
            "[x](/tmp/../a b.md)",
        ] {
            let runs = paragraph(source);
            assert_eq!(link_of(&runs), None, "{source}");
            assert!(text(&runs).contains('('), "{source} should stay literal");
        }
    }

    #[test]
    fn already_bracketed_destinations_are_left_alone() {
        let runs = paragraph("see [it](/tmp/Some Folder/it's here.txt) now");
        assert_eq!(text(&runs), "see it now");
        assert_eq!(link_of(&runs), Some("/tmp/Some Folder/it's here.txt"));
    }

    #[test]
    fn code_keeps_its_spelling() {
        let tree = parse_full("```\n[x](/tmp/a b.md)\n```\n");
        let Block::CodeBlock { code, .. } = &tree.blocks[0].block else {
            panic!("fenced code");
        };
        assert_eq!(code.trim_end(), "[x](/tmp/a b.md)");

        let tree = parse_full("    [x](/tmp/a b.md)\n");
        let Block::CodeBlock { code, .. } = &tree.blocks[0].block else {
            panic!("indented code");
        };
        assert_eq!(code.trim_end(), "[x](/tmp/a b.md)");

        let runs = paragraph("`[x](/tmp/a b.md)`");
        assert!(runs.iter().all(|run| run.style.link.is_none()));
        assert_eq!(text(&runs), "[x](/tmp/a b.md)");
    }

    #[test]
    fn unclosed_destinations_wait_for_their_closing_paren() {
        let mut parser = IncrementalParser::new();
        parser.append("see [x](/tmp/a b");
        assert!(link_of(&paragraph(parser.source())).is_none());
        parser.append(".md) now");
        assert_eq!(link_of(&paragraph(parser.source())), Some("/tmp/a b.md"));
        assert_eq!(parser.tree(), &parse_full(parser.source()));
    }

    #[test]
    fn rewritten_ranges_map_back_to_the_original_source() {
        let source = "intro [x](/tmp/a b.md) tail\n\nsecond paragraph\n";
        let tree = parse_full(source);
        assert_eq!(tree.blocks.len(), 2);
        assert_eq!(
            &source[tree.blocks[0].range.clone()],
            "intro [x](/tmp/a b.md) tail\n"
        );
        assert_eq!(&source[tree.blocks[1].range.clone()], "second paragraph\n");
    }

    #[test]
    fn rewrite_is_idempotent_on_its_own_output() {
        let source = "see [it](/tmp/Some Folder/it's here.txt) and [web](https://x.dev/a b)";
        let once = rewrite_absolute_destinations(source).expect("rewrite");
        assert!(once.0.contains("](</tmp/Some Folder/it's here.txt>)"));
        assert!(once.0.contains("https://x.dev/a b"));
        assert!(
            rewrite_absolute_destinations(&once.0).is_none(),
            "second pass must find nothing to rewrite"
        );
    }
}
