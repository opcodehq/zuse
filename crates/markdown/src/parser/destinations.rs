//! Preserve workspace paths with spaces in Markdown link destinations.

use std::{collections::HashSet, ops::Range};

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
    let mut unclosed = HashSet::new();
    // Bytes before this are already in `rewritten`; skipped regions advance
    // it without copying, so the next splice stays contiguous.
    let mut copied = 0usize;
    let mut at = 0usize;
    let mut protected_ranges = pulldown_cmark::Parser::new_ext(source, super::options())
        .into_offset_iter()
        .filter_map(|(event, range)| {
            use pulldown_cmark::{Event, Tag};
            matches!(
                event,
                Event::Start(
                    Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::Link { .. } | Tag::Image { .. }
                ) | Event::Code(_)
            )
            .then_some((
                range,
                matches!(event, Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock)),
            ))
        })
        .peekable();
    let mut labels = 0;
    while at < bytes.len() {
        while protected_ranges
            .peek()
            .is_some_and(|(range, _)| range.end <= at)
        {
            protected_ranges.next();
        }
        if protected_ranges
            .peek()
            .is_some_and(|(range, _)| range.start <= at)
        {
            let (range, block) = protected_ranges.next().unwrap();
            let end = range.end;
            if block {
                labels = 0;
            }
            skip(&mut rewritten, source, &mut copied, end);
            at = end;
            continue;
        }
        if bytes[at] == b'\n'
            && bytes[..at]
                .rsplit(|&b| b == b'\n')
                .next()
                .is_some_and(|line| line.iter().all(u8::is_ascii_whitespace))
        {
            labels = 0;
        }
        match bytes[at] {
            b'[' if !is_escaped(bytes, at) => {
                labels += 1;
                at += 1;
            }
            b']' if !is_escaped(bytes, at) => {
                if labels == 0 {
                    at += 1;
                    continue;
                }
                labels -= 1;
                if bytes.get(at + 1) != Some(&b'(') {
                    at += 1;
                    continue;
                }
                let open = at + 1;
                let start = skip_destination_spaces(bytes, open + 1);
                // Valid CommonMark links (including their titles and angle
                // bracket destinations) are already protected by the parser.
                let tail = &bytes[start..];
                if !(tail.starts_with(b"/") || tail.starts_with(b"file://"))
                    || unclosed.contains(&open)
                {
                    at += 1;
                    continue;
                }
                let Some((path_end, close)) = destination_ends(bytes, open, start, &mut unclosed)
                else {
                    at += 2;
                    continue;
                };
                let destination = source[start..path_end].trim_end_matches(' ');
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
                    at = close + 1;
                } else {
                    at += 1;
                }
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

/// An odd number of backslashes before `at` escapes the byte there.
pub(super) fn is_escaped(bytes: &[u8], at: usize) -> bool {
    bytes[..at]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 1
}

fn skip_destination_spaces(bytes: &[u8], at: usize) -> usize {
    let mut at = at;
    while bytes.get(at) == Some(&b' ') {
        at += 1;
    }
    at
}

/// Find the end of the path and the closing link delimiter. Remember the
/// openers left unmatched at the end of a scan, so later candidates in that
/// suffix do not repeat it. Balanced inner candidates can still be rewritten.
fn destination_ends(
    bytes: &[u8],
    open: usize,
    start: usize,
    unclosed: &mut HashSet<usize>,
) -> Option<(usize, usize)> {
    let mut nested = Vec::new();
    let mut at = start;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'\\' => at += 2,
            b'(' => {
                nested.push(at);
                at += 1;
            }
            b')' => {
                if nested.pop().is_none() {
                    return Some((at, at));
                }
                at += 1;
            }
            b' ' | b'\t' => {
                let tail = skip_title_spaces(bytes, at);
                let link_level = nested
                    .last()
                    .is_none_or(|&open| open > 0 && bytes[open - 1] == b']');
                if link_level && let Some(close) = title_end(bytes, tail) {
                    if nested.pop().is_none() {
                        return Some((at, close));
                    }
                    at = close + 1;
                } else if bytes[at..tail].contains(&b'\t') {
                    break;
                } else {
                    at = tail;
                }
            }
            byte if byte.is_ascii_control() => break,
            _ => at += 1,
        }
    }
    unclosed.insert(open);
    unclosed.extend(nested);
    None
}

fn skip_title_spaces(bytes: &[u8], at: usize) -> usize {
    at + bytes[at..]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

/// A title is separate from the filename only when it is delimited and
/// followed by the link's closing paren. This leaves apostrophes and balanced
/// parentheses that are part of the path alone.
fn title_end(bytes: &[u8], start: usize) -> Option<usize> {
    let delimiter = *bytes.get(start)?;
    let closing = match delimiter {
        b'(' => b')',
        b'\'' | b'"' => delimiter,
        _ => return None,
    };
    let mut at = start + 1;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'\\' => at += 2,
            byte if byte == closing => {
                let close = skip_title_spaces(bytes, at + 1);
                let close = if bytes.get(close) == Some(&b'\n') {
                    skip_title_spaces(bytes, close + 1)
                } else {
                    close
                };
                return (bytes.get(close) == Some(&b')')).then_some(close);
            }
            b'(' if delimiter == b'(' => return None,
            b'\n' => {
                at = skip_title_spaces(bytes, at + 1);
                if bytes.get(at) == Some(&b'\n') {
                    return None;
                }
            }
            b'\t' => at += 1,
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
    fn titles_stay_separate_from_absolute_paths() {
        for title in [
            r#""With ) in the title""#,
            "'Single quotes'",
            "(Parentheses)",
        ] {
            let link = format!("[x](/tmp/a.md {title})");
            assert!(rewrite_absolute_destinations(&link).is_none(), "{link}");
            assert_eq!(link_of(&paragraph(&link)), Some("/tmp/a.md"));

            let image = format!("![x](/tmp/a picture.png {title})");
            let (rewritten, _) = rewrite_absolute_destinations(&image).unwrap();
            assert_eq!(rewritten, format!("![x](</tmp/a picture.png> {title})"));
            let runs = paragraph(&image);
            let image = runs
                .iter()
                .find_map(|run| run.style.image.as_ref())
                .unwrap();
            assert_eq!(image.source, "/tmp/a picture.png");
            assert_eq!(image.title, &title[1..title.len() - 1]);
        }
    }

    #[test]
    fn titles_can_contain_link_spelling_and_line_breaks() {
        let source = r#"[x](relative.md "A literal [y](/tmp/a b.md)")"#;
        assert!(rewrite_absolute_destinations(source).is_none());

        let source = "[x](/tmp/a.md \"Title with )\nand a second line\")";
        assert!(rewrite_absolute_destinations(source).is_none());
        assert_eq!(link_of(&paragraph(source)), Some("/tmp/a.md"));
    }

    #[test]
    fn unmatched_openers_are_recorded_without_losing_balanced_links() {
        let source = "[outer](/tmp/unfinished [inner](/tmp/a b.md) [tail](/tmp/unfinished";
        let open = source.find('(').unwrap();
        let mut unclosed = HashSet::new();
        assert!(destination_ends(source.as_bytes(), open, open + 1, &mut unclosed).is_none());
        let tail = source.rfind('(').unwrap();
        assert_eq!(unclosed, HashSet::from([open, tail]));
        assert_eq!(
            rewrite_absolute_destinations(source).unwrap().0,
            "[outer](/tmp/unfinished [inner](</tmp/a b.md>) [tail](/tmp/unfinished"
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
        let nested = "[x](</tmp/a [y](/tmp/b c.md>)";
        assert!(rewrite_absolute_destinations(nested).is_none());
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
