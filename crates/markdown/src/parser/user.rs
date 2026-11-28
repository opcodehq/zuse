//! Preserve the line breaks and leading indentation of sent prompts.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use pulldown_cmark::{CodeBlockKind, Event, RefDefs, Tag};

use super::{BlockTree, tree_from_events, with_events};

/// Parse a message the user sent. A user types prose, not Markdown layout, so
/// unlike [`super::parse_full`]:
/// - every newline they typed stays a line break (soft breaks become hard);
/// - indentation at the start of a top-level paragraph line is kept as a
///   display-only run, shown but never copied;
/// - a top-level run of lines indented four or more spaces stays prose, not
///   an indented code block. Fenced code is unchanged.
pub fn parse_user(source: &str) -> BlockTree {
    with_events(source, 0, None, |events, refs, mut closed_fences| {
        let (events, indents) = user_events(source, events, refs, &mut closed_fences);
        tree_from_events(&events, Some(&indents), &closed_fences)
    })
}

type Events<'e> = Vec<(Event<'e>, Range<usize>)>;

fn user_events<'e>(
    source: &str,
    events: Events<'e>,
    refs: &RefDefs<'_>,
    closed_fences: &mut HashSet<usize>,
) -> (Events<'e>, HashMap<usize, String>) {
    // Top-level indented code reparses as its lines without the indentation.
    let mut spliced = Vec::with_capacity(events.len());
    let mut depth = 0usize;
    let mut events = events.into_iter();
    while let Some((event, range)) = events.next() {
        if depth == 0 && matches!(event, Event::Start(Tag::CodeBlock(CodeBlockKind::Indented))) {
            let mut end = range.end;
            for (event, range) in events.by_ref() {
                if matches!(event, Event::End(_)) {
                    end = end.max(range.end);
                    break;
                }
            }
            spliced.extend(reparse_dedented(
                source,
                range.start..end,
                refs,
                closed_fences,
            ));
            continue;
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        spliced.push((event, range));
    }

    let mut out = Vec::with_capacity(spliced.len());
    let mut indents = HashMap::new();
    let mut depth = 0usize;
    let mut paragraph = false;
    let mut line_start = false;
    for (event, range) in spliced {
        match &event {
            Event::Start(Tag::Paragraph) if depth == 0 => {
                paragraph = true;
                line_start = true;
            }
            Event::SoftBreak | Event::HardBreak if paragraph => line_start = true,
            Event::End(_) => {}
            _ if line_start => {
                if let Some(indent) = leading_indent(source, range.start) {
                    indents.insert(out.len(), indent);
                }
                line_start = false;
            }
            _ => {}
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    paragraph = false;
                    line_start = false;
                }
            }
            _ => {}
        }
        let event = match event {
            Event::SoftBreak => Event::HardBreak,
            event => event,
        };
        out.push((event, range));
    }
    (out, indents)
}

/// Events for the lines of `range` with their leading whitespace removed,
/// ranges mapped back into `source` (so each line still finds its indent).
fn reparse_dedented(
    source: &str,
    range: Range<usize>,
    refs: &RefDefs<'_>,
    closed_fences: &mut HashSet<usize>,
) -> Events<'static> {
    let start = source[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let mut dedented = String::new();
    // (line start in `dedented`, its first non-blank byte in `source`)
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut at = start;
    for line in source[start..range.end].split_inclusive('\n') {
        let trimmed = line.trim_start_matches([' ', '\t']);
        lines.push((dedented.len(), at + line.len() - trimmed.len()));
        dedented.push_str(trimmed);
        at += line.len();
    }
    let original = |offset: usize| {
        let line = lines.partition_point(|&(from, _)| from <= offset) - 1;
        lines[line].1 + offset - lines[line].0
    };
    with_events(&dedented, 0, Some(refs), |events, _, fences| {
        closed_fences.extend(fences.into_iter().map(original));
        events
            .into_iter()
            .map(|(event, range)| {
                (
                    event.into_static(),
                    original(range.start)..original(range.end),
                )
            })
            .collect()
    })
}

/// The spaces and tabs between the start of `at`'s line and `at`, if that is
/// all there is. Tabs show as four spaces.
fn leading_indent(source: &str, at: usize) -> Option<String> {
    let start = source[..at].rfind('\n').map_or(0, |at| at + 1);
    let indent = &source[start..at];
    (!indent.is_empty() && indent.bytes().all(|b| b == b' ' || b == b'\t'))
        .then(|| indent.replace('\t', "    "))
}
