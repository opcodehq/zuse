//! Block-level markdown parsing over pulldown-cmark.
//!
//! Full parses build a [`BlockTree`] — a list of top-level blocks with their
//! byte ranges in the source. The streaming path ([`IncrementalParser`]) reparses
//! only from the last stable top-level block boundary: text before the start of
//! the last top-level block cannot be affected by an append, so each streamed
//! delta costs roughly O(delta + last block) instead of O(document).
//!
//! Soundness guard: link-reference definitions (`[label]: url`) have non-local
//! effects (a definition anywhere resolves references anywhere), so a source
//! containing one drops to full reparses. The parity unit tests stream corpora
//! through both paths and assert equality.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag,
};

mod destinations;
mod html;
mod incremental;
mod user;

#[cfg(test)]
mod tests;

use destinations::rewrite_absolute_destinations;
use html::{HtmlInline, HtmlToken, Sink, html_block_text, parse_html_tag};

pub use incremental::IncrementalParser;
pub use user::parse_user;

// ---------------------------------------------------------------------------
// Tree model
// ---------------------------------------------------------------------------

/// Inline styling flags threaded through nested emphasis/links.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strikethrough: bool,
    /// Destination URL when inside a link.
    pub link: Option<String>,
    /// The label to show instead of `text` for a file link whose text is the
    /// path itself (an inline-code span) rather than a label the author
    /// wrote. Never set by the parser; `None` shows `text`. When set, copy
    /// and selection still map back to `text`.
    pub file_label: Option<String>,
    pub image: Option<InlineImage>,
    pub task: Option<TaskMarker>,
    /// Shown but never copied or selected: the indentation a user typed at
    /// the start of a line in a sent message ([`parse_user`]). Never set by
    /// [`parse_full`].
    pub display_only: bool,
    /// The host's inline token index, assigned when projecting a sent message.
    pub user_literal: Option<usize>,
    /// A footnote reference (`[^label]`); the run's text is `[label]`.
    pub footnote: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMarker {
    pub checked: bool,
    /// Byte range of `[ ]`, `[x]` or `[X]` in the original document.
    pub range: Range<usize>,
}

/// An image remains inline in the AST; hosts can opt in to visual media.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImage {
    pub source: String,
    pub alt: String,
    pub title: String,
    pub link: Option<String>,
}

/// One run of identically-styled inline text.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineRun {
    pub text: String,
    pub style: InlineStyle,
}

/// A markdown block. Containers nest.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph {
        runs: Vec<InlineRun>,
    },
    Heading {
        level: u8,
        runs: Vec<InlineRun>,
    },
    CodeBlock {
        language: Option<String>,
        /// Fence title from the info string (`title="..."`, `title=...` or
        /// `filename=...`), shown in place of the language label.
        title: Option<String>,
        code: String,
    },
    BlockQuote {
        /// A GFM alert (`> [!NOTE]` …); `None` is an ordinary quote.
        kind: Option<AlertKind>,
        children: Vec<Block>,
    },
    /// An HTML `<details>` disclosure: the `<summary>` label (empty when the
    /// author gave none) over a body that starts collapsed.
    Details {
        summary: Vec<InlineRun>,
        children: Vec<Block>,
    },
    /// A footnote definition (`[^label]: …`). `section_start` marks the first
    /// of a run of consecutive definitions, which opens the footnote section.
    FootnoteDefinition {
        label: String,
        children: Vec<Block>,
        section_start: bool,
    },
    List {
        ordered_start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Table {
        header: Vec<Vec<InlineRun>>,
        rows: Vec<Vec<Vec<InlineRun>>>,
        /// Per-column GFM alignment (`:--`/`:-:`/`--:`); unspecified is Left.
        align: Vec<TableAlign>,
    },
    Rule,
}

/// The five GFM alert kinds (`> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`,
/// `[!WARNING]`, `[!CAUTION]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }
}

impl From<BlockQuoteKind> for AlertKind {
    fn from(kind: BlockQuoteKind) -> Self {
        match kind {
            BlockQuoteKind::Note => Self::Note,
            BlockQuoteKind::Tip => Self::Tip,
            BlockQuoteKind::Important => Self::Important,
            BlockQuoteKind::Warning => Self::Warning,
            BlockQuoteKind::Caution => Self::Caution,
        }
    }
}

/// GFM column alignment for a table (mdast `align`; `None` renders as Left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// A top-level block plus its byte range in the source. The range start is the
/// stable-boundary anchor for incremental reparses.
#[derive(Debug, Clone, PartialEq)]
pub struct TopBlock {
    pub range: Range<usize>,
    pub block: Block,
}

/// The parse result: top-level blocks in document order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockTree {
    // Completed blocks are immutable. Canonical/display trees and old/new
    // transcript rows share their contents across streamed tail updates.
    pub blocks: Vec<Arc<TopBlock>>,
}

impl BlockTree {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }
}

// ---------------------------------------------------------------------------
// Full parse
// ---------------------------------------------------------------------------

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
}

/// Parse a whole source into a [`BlockTree`].
pub fn parse_full(source: &str) -> BlockTree {
    parse_at(source, 0)
}

fn parse_at(source: &str, offset: usize) -> BlockTree {
    with_events(source, offset, |events| tree_from_events(&events, None))
}

/// Run `build` over `source`'s pulldown events, with ranges in `source`.
fn with_events<R>(
    source: &str,
    offset: usize,
    build: impl FnOnce(Vec<(Event, Range<usize>)>) -> R,
) -> R {
    let (text, insertions) = match rewrite_absolute_destinations(source) {
        Some((text, insertions)) => (Cow::Owned(text), Some(insertions)),
        None => (Cow::Borrowed(source), None),
    };
    let events: Vec<(Event, Range<usize>)> = Parser::new_ext(&text, options())
        .into_offset_iter()
        .map(|(event, range)| {
            // Inserted angle brackets shift every following offset; ranges
            // come from the rewritten text and must point into the original.
            let range = match &insertions {
                Some(insertions) => insertions.original_range(range),
                None => range,
            };
            (event, range.start + offset..range.end + offset)
        })
        .collect();
    build(events)
}

fn tree_from_events(
    events: &[(Event, Range<usize>)],
    indents: Option<&HashMap<usize, String>>,
) -> BlockTree {
    let mut cur = Cursor {
        events,
        ix: 0,
        indents,
        details_closes: 0,
    };
    let mut sink = Sink::default();
    while let Some((event, range)) = cur.peek() {
        let range = range.clone();
        match event {
            Event::Rule => {
                cur.bump();
                sink.block(Block::Rule, range);
            }
            Event::Start(Tag::HtmlBlock) => {
                let text = html_block_text(&mut cur);
                sink.html(&text, range);
            }
            Event::Start(_) => {
                let closes = cur.details_closes;
                for block in parse_started_block(&mut cur) {
                    sink.block(block, range.clone());
                }
                sink.close_inline(cur.details_closes - closes, range.end);
            }
            // Stray inline events at top level (shouldn't happen): skip.
            _ => cur.bump(),
        }
    }
    let mut blocks: Vec<Arc<TopBlock>> = Vec::new();
    let mut in_footnotes = false;
    for (mut block, range) in sink.finish() {
        let footnote = matches!(block, Block::FootnoteDefinition { .. });
        if let Block::FootnoteDefinition { section_start, .. } = &mut block {
            *section_start = !in_footnotes;
        }
        in_footnotes = footnote;
        blocks.push(Arc::new(TopBlock { range, block }));
    }
    BlockTree { blocks }
}

struct Cursor<'a, 'e> {
    events: &'a [(Event<'e>, Range<usize>)],
    ix: usize,
    /// Display-only indentation shown before the inline event at an index
    /// ([`parse_user`] only).
    indents: Option<&'a HashMap<usize, String>>,
    /// `</details>` tags seen in inline content so far; the enclosing
    /// [`Sink`] closes that many disclosures after the block.
    details_closes: usize,
}

impl<'a, 'e> Cursor<'a, 'e> {
    fn peek(&self) -> Option<&(Event<'e>, Range<usize>)> {
        self.events.get(self.ix)
    }

    fn peek_event(&self) -> Option<&Event<'e>> {
        self.peek().map(|(e, _)| e)
    }

    fn bump(&mut self) {
        self.ix += 1;
    }

    fn next_event(&mut self) -> Option<Event<'e>> {
        let event = self.events.get(self.ix).map(|(e, _)| e.clone());
        if event.is_some() {
            self.ix += 1;
        }
        event
    }
}

fn is_block_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::CodeBlock(_)
            | Tag::BlockQuote(_)
            | Tag::List(_)
            | Tag::Item
            | Tag::Table(_)
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
    )
}

/// Split a fence info string into its language (first word, unless that word
/// is itself an attribute) and an optional title from `title="..."`,
/// `title=...` or `filename=...` (T3 also reads `file=`). Single or double
/// quotes may wrap a value with spaces; `title` wins over `filename`.
pub fn parse_fence_info(info: &str) -> (Option<String>, Option<String>) {
    let info = info.trim();
    let mut language = None;
    let mut title = None;
    let mut filename = None;
    let mut rest = info;
    let mut first = true;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let eq = rest[..word_end].find('=');
        let Some(eq) = eq else {
            if first {
                language = Some(rest[..word_end].to_string());
            }
            first = false;
            rest = &rest[word_end..];
            continue;
        };
        first = false;
        let key = rest[..eq].to_ascii_lowercase();
        let after = &rest[eq + 1..];
        let (value, next) = match after.chars().next() {
            Some(q @ ('"' | '\'')) => match after[1..].find(q) {
                Some(close) => (&after[1..1 + close], &after[2 + close..]),
                None => (&after[1..], ""),
            },
            _ => {
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        let value = value.trim();
        if !value.is_empty() {
            match key.as_str() {
                "title" if title.is_none() => title = Some(value.to_string()),
                "filename" | "file" if filename.is_none() => filename = Some(value.to_string()),
                _ => {}
            }
        }
        rest = next;
    }
    (language, title.or(filename))
}

/// Consume a `Start(tag)` and everything through its matching `End`, producing
/// block(s). Unknown containers are transparent (children splice in).
fn parse_started_block(cur: &mut Cursor) -> Vec<Block> {
    let Some(Event::Start(tag)) = cur.next_event() else {
        return Vec::new();
    };
    match tag {
        Tag::Paragraph => {
            if let Some(code) = sole_display_math(cur) {
                return vec![Block::CodeBlock {
                    language: Some("math".into()),
                    title: None,
                    code,
                }];
            }
            vec![Block::Paragraph {
                runs: parse_inline_container(cur, &InlineStyle::default()),
            }]
        }
        Tag::Heading { level, .. } => vec![Block::Heading {
            level: heading_level(level),
            runs: parse_inline_container(cur, &InlineStyle::default()),
        }],
        Tag::CodeBlock(kind) => {
            let (language, title) = match kind {
                CodeBlockKind::Fenced(info) => parse_fence_info(&info),
                CodeBlockKind::Indented => (None, None),
            };
            let mut code = String::new();
            loop {
                match cur.next_event() {
                    Some(Event::Text(t)) => code.push_str(&t),
                    Some(Event::End(_)) | None => break,
                    Some(_) => {}
                }
            }
            // Fenced blocks carry a trailing newline; render per-line without it.
            if code.ends_with('\n') {
                code.pop();
            }
            vec![Block::CodeBlock {
                language,
                title,
                code,
            }]
        }
        Tag::BlockQuote(kind) => vec![Block::BlockQuote {
            kind: kind.map(AlertKind::from),
            children: parse_block_sequence(cur),
        }],
        Tag::FootnoteDefinition(label) => vec![Block::FootnoteDefinition {
            label: label.into_string(),
            children: parse_block_sequence(cur),
            section_start: true,
        }],
        Tag::List(ordered_start) => {
            let mut items = Vec::new();
            loop {
                match cur.peek_event() {
                    Some(Event::Start(Tag::Item)) => {
                        cur.bump();
                        items.push(parse_block_sequence(cur));
                    }
                    Some(Event::End(_)) | None => {
                        cur.bump();
                        break;
                    }
                    Some(_) => cur.bump(),
                }
            }
            vec![Block::List {
                ordered_start,
                items,
            }]
        }
        Tag::Table(align) => {
            let align = align
                .iter()
                .map(|a| match a {
                    Alignment::Center => TableAlign::Center,
                    Alignment::Right => TableAlign::Right,
                    Alignment::None | Alignment::Left => TableAlign::Left,
                })
                .collect();
            vec![parse_table(cur, align)]
        }
        Tag::HtmlBlock => {
            // Sequences route HTML blocks through their `Sink`; this is the
            // same treatment for a lone block.
            cur.ix -= 1;
            let text = html_block_text(cur);
            let mut sink = Sink::default();
            sink.html(&text, 0..0);
            sink.finish().into_iter().map(|(block, _)| block).collect()
        }
        // Transparent containers.
        _ => parse_block_sequence(cur),
    }
}

/// A paragraph that is only display math (`$$…$$`, whitespace around it)
/// renders as a `math` code block. Consumes the paragraph when it is one.
fn sole_display_math(cur: &mut Cursor) -> Option<String> {
    let mut math: Vec<&str> = Vec::new();
    let mut at = cur.ix;
    loop {
        match &cur.events.get(at)?.0 {
            Event::DisplayMath(t) => math.push(t),
            Event::Text(t) if t.trim().is_empty() => {}
            Event::SoftBreak | Event::HardBreak => {}
            Event::End(_) => break,
            _ => return None,
        }
        at += 1;
    }
    if math.is_empty() {
        return None;
    }
    let code = math
        .iter()
        .map(|t| t.trim_matches(|c| c == '\n' || c == '\r'))
        .collect::<Vec<_>>()
        .join("\n");
    cur.ix = at + 1;
    Some(code)
}

/// Parse a block sequence until the container's `End` (consumed). Bare inline
/// events (tight list items) accumulate into an implicit paragraph.
fn parse_block_sequence(cur: &mut Cursor) -> Vec<Block> {
    let mut sink = Sink::default();
    let mut inline_acc: Vec<InlineRun> = Vec::new();
    let mut html = HtmlInline::default();
    let mut closes = cur.details_closes;
    while let Some(event) = cur.peek_event() {
        match event {
            Event::End(_) => {
                cur.bump();
                break;
            }
            Event::Start(Tag::HtmlBlock) => {
                flush_paragraph(&mut sink, &mut inline_acc, cur, &mut closes);
                let text = html_block_text(cur);
                sink.html(&text, 0..0);
            }
            Event::Start(tag) if is_block_tag(tag) => {
                flush_paragraph(&mut sink, &mut inline_acc, cur, &mut closes);
                for block in parse_started_block(cur) {
                    sink.block(block, 0..0);
                }
                sink.close_inline(cur.details_closes - closes, 0);
                closes = cur.details_closes;
            }
            Event::Rule => {
                flush_paragraph(&mut sink, &mut inline_acc, cur, &mut closes);
                cur.bump();
                sink.block(Block::Rule, 0..0);
            }
            _ => parse_inline_event(cur, &mut inline_acc, &InlineStyle::default(), &mut html),
        }
    }
    flush_paragraph(&mut sink, &mut inline_acc, cur, &mut closes);
    sink.finish().into_iter().map(|(block, _)| block).collect()
}

fn flush_paragraph(sink: &mut Sink, acc: &mut Vec<InlineRun>, cur: &Cursor, closes: &mut usize) {
    if !acc.is_empty() {
        sink.block(
            Block::Paragraph {
                runs: merge_runs(std::mem::take(acc)),
            },
            0..0,
        );
    }
    sink.close_inline(cur.details_closes - *closes, 0);
    *closes = cur.details_closes;
}

fn parse_table(cur: &mut Cursor, align: Vec<TableAlign>) -> Block {
    let mut header = Vec::new();
    let mut rows = Vec::new();
    loop {
        match cur.peek_event() {
            Some(Event::Start(Tag::TableHead)) => {
                cur.bump();
                header = parse_table_cells(cur);
            }
            Some(Event::Start(Tag::TableRow)) => {
                cur.bump();
                rows.push(parse_table_cells(cur));
            }
            Some(Event::End(_)) | None => {
                cur.bump();
                break;
            }
            Some(_) => cur.bump(),
        }
    }
    Block::Table {
        header,
        rows,
        align,
    }
}

fn parse_table_cells(cur: &mut Cursor) -> Vec<Vec<InlineRun>> {
    let mut cells = Vec::new();
    loop {
        match cur.peek_event() {
            Some(Event::Start(Tag::TableCell)) => {
                cur.bump();
                cells.push(parse_inline_container(cur, &InlineStyle::default()));
            }
            Some(Event::End(_)) | None => {
                cur.bump();
                break;
            }
            Some(_) => cur.bump(),
        }
    }
    cells
}

/// Parse inline events until the container's `End` (consumed).
fn parse_inline_container(cur: &mut Cursor, style: &InlineStyle) -> Vec<InlineRun> {
    let mut runs = Vec::new();
    let mut html = HtmlInline::default();
    while let Some(event) = cur.peek_event() {
        if matches!(event, Event::End(_)) {
            cur.bump();
            break;
        }
        parse_inline_event(cur, &mut runs, style, &mut html);
    }
    // Autolink AFTER merging: pulldown splits Text events at would-be
    // emphasis chars ("…/Foo_(bar)" arrives as three events), so scanning
    // per-event would truncate URLs at every underscore.
    autolink_runs(merge_runs(runs))
}

fn parse_inline_event(
    cur: &mut Cursor,
    runs: &mut Vec<InlineRun>,
    base: &InlineStyle,
    html: &mut HtmlInline,
) {
    let range = cur.peek().map(|(_, range)| range.clone());
    if let Some(indent) = cur.indents.and_then(|indents| indents.get(&cur.ix)) {
        runs.push(InlineRun {
            text: indent.clone(),
            style: InlineStyle {
                display_only: true,
                ..InlineStyle::default()
            },
        });
    }
    let Some(event) = cur.next_event() else {
        return;
    };
    let style = &html.style(base);
    let push = |runs: &mut Vec<InlineRun>, text: String, style: InlineStyle| {
        if !text.is_empty() {
            runs.push(InlineRun { text, style });
        }
    };
    match event {
        Event::Text(t) => push(runs, t.into_string(), style.clone()),
        Event::Code(t) => {
            let mut s = style.clone();
            s.code = true;
            push(runs, t.into_string(), s);
        }
        Event::SoftBreak => push(runs, " ".into(), style.clone()),
        Event::HardBreak => push(runs, "\n".into(), style.clone()),
        Event::Html(t) | Event::InlineHtml(t) => {
            // One tag (or comment) per event: styling tags restyle what
            // follows, `<br>` breaks the line, everything else vanishes.
            if let Some((token, _)) = parse_html_tag(&t) {
                if matches!(&token, HtmlToken::Tag { name, closing: true, .. } if name == "details")
                {
                    cur.details_closes += 1;
                }
                if let Some(text) = html.apply(&token, false) {
                    push(runs, text.into(), html.style(base));
                }
            }
        }
        Event::InlineMath(t) | Event::DisplayMath(t) => {
            let mut s = style.clone();
            s.code = true;
            let text = t.trim().replace(['\n', '\r'], " ");
            push(runs, text, s);
        }
        Event::TaskListMarker(done) => {
            let mut style = style.clone();
            style.task = Some(TaskMarker {
                checked: done,
                range: range.unwrap(),
            });
            push(
                runs,
                if done { "[x] ".into() } else { "[ ] ".into() },
                style,
            );
        }
        Event::FootnoteReference(t) => {
            let mut s = style.clone();
            s.footnote = true;
            push(runs, format!("[{t}]"), s);
        }
        Event::Start(tag) => {
            let mut inner = style.clone();
            match tag {
                Tag::Emphasis => inner.italic = true,
                Tag::Strong => inner.bold = true,
                Tag::Strikethrough => inner.strikethrough = true,
                Tag::Image {
                    dest_url, title, ..
                } => {
                    let alt: String = parse_inline_container(cur, style)
                        .iter()
                        .map(|run| run.text.as_str())
                        .collect();
                    inner.image = Some(InlineImage {
                        source: dest_url.to_string(),
                        alt: alt.clone(),
                        title: title.to_string(),
                        link: style.link.clone(),
                    });
                    inner.link = Some(dest_url.into_string());
                    runs.push(InlineRun {
                        text: alt,
                        style: inner,
                    });
                    return;
                }
                Tag::Link { dest_url, .. } => {
                    inner.link = Some(dest_url.into_string());
                }
                _ => {}
            }
            runs.extend(parse_inline_container(cur, &inner));
        }
        // `End` is consumed by the container loop; anything else is ignored.
        _ => {}
    }
}

/// Promote bare `http(s)://` URLs into link runs — GFM's autolink extension,
/// which pulldown-cmark has no option for (agents paste naked PR/issue URLs
/// constantly; user report: the link isn't clickable). Runs already inside a
/// link or code span pass through untouched. Idempotent, so nested containers
/// re-applying it on their merged output is harmless.
fn autolink_runs(runs: Vec<InlineRun>) -> Vec<InlineRun> {
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        if run.style.link.is_some() || run.style.code {
            out.push(run);
        } else {
            push_text_autolinked(&mut out, &run.text, &run.style);
        }
    }
    out
}

fn push_text_autolinked(runs: &mut Vec<InlineRun>, text: &str, style: &InlineStyle) {
    let push = |runs: &mut Vec<InlineRun>, text: &str, style: InlineStyle| {
        if !text.is_empty() {
            runs.push(InlineRun {
                text: text.to_string(),
                style,
            });
        }
    };
    let mut rest = text;
    while let Some(at) = find_url_start(rest) {
        let from = &rest[at..];
        let scheme = if from.starts_with("https://") {
            "https://".len()
        } else {
            "http://".len()
        };
        let len = bare_url_len(from);
        if len <= scheme {
            // A scheme with nothing after it stays text (don't re-find it).
            push(runs, &rest[..at + scheme], style.clone());
            rest = &from[scheme..];
            continue;
        }
        push(runs, &rest[..at], style.clone());
        let mut linked = style.clone();
        linked.link = Some(from[..len].to_string());
        push(runs, &from[..len], linked);
        rest = &from[len..];
    }
    push(runs, rest, style.clone());
}

/// First viable `http(s)://` occurrence: not glued to a preceding
/// alphanumeric (`foohttps://…` stays text, per GFM's boundary rule).
fn find_url_start(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = text[from..].find("http") {
        let at = from + rel;
        let after = &text[at..];
        let is_scheme = after.starts_with("http://") || after.starts_with("https://");
        let boundary = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        if is_scheme && boundary {
            return Some(at);
        }
        from = at + "http".len();
    }
    None
}

/// Byte length of the bare URL at the start of `text`: run to whitespace (or
/// a delimiter that never appears in pasted URLs), then trim the trailing
/// punctuation GFM excludes — a closing paren only stays when an opener
/// inside the URL balances it ("…/Foo_(bar))" keeps one, sheds one).
fn bare_url_len(text: &str) -> usize {
    let end = text
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
        .map_or(text.len(), |(i, _)| i);
    let mut url = &text[..end];
    while let Some(last) = url.chars().next_back() {
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_' | '~' => true,
            ')' => url.matches('(').count() < url.matches(')').count(),
            _ => false,
        };
        if !trim {
            break;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
    url.len()
}

/// Merge adjacent identically-styled runs (keeps run counts small and makes the
/// tree canonical for equality tests).
fn merge_runs(runs: Vec<InlineRun>) -> Vec<InlineRun> {
    let mut out: Vec<InlineRun> = Vec::with_capacity(runs.len());
    for run in runs {
        match out.last_mut() {
            Some(last) if last.style == run.style && run.style.image.is_none() => {
                last.text.push_str(&run.text)
            }
            _ => out.push(run),
        }
    }
    out
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}
