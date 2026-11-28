//! Decode and group the HTML constructs retained by the Markdown model.

use std::{borrow::Cow, ops::Range, sync::Arc};

use pulldown_cmark::Event;

use super::{Block, Cursor, InlineRun, InlineStyle, autolink_runs, merge_runs, parse_at};

/// Collects a block sequence while grouping `<details>` regions. CommonMark
/// splits `<details><summary>…</summary>` + markdown body + `</details>` into
/// separate HTML blocks around ordinary blocks, so the grouping happens here,
/// above pulldown-cmark: an open frame swallows blocks until its close tag
/// (in an HTML block or inline), and an unclosed one ends with the sequence —
/// which is what a streaming `<details>` looks like until its close arrives.
#[derive(Default)]
pub(super) struct Sink {
    out: Vec<(Block, Range<usize>)>,
    frames: Vec<DetailsFrame>,
    end: usize,
}

struct DetailsFrame {
    start: usize,
    summary: Option<Vec<InlineRun>>,
    /// Raw summary text while its `<summary>` is still open.
    summary_raw: Option<String>,
    children: Vec<Block>,
}

impl Sink {
    pub(super) fn block(&mut self, block: Block, range: Range<usize>) {
        self.end = self.end.max(range.end);
        self.finish_summary();
        match self.frames.last_mut() {
            Some(frame) => frame.children.push(block),
            None => self.out.push((block, range)),
        }
    }

    fn finish_summary(&mut self) {
        if let Some(frame) = self.frames.last_mut()
            && let Some(raw) = frame.summary_raw.take()
        {
            frame.summary = Some(summary_runs(&raw));
        }
    }

    /// One raw HTML block: comments vanish, details/summary tags drive the
    /// frames, and everything else becomes styled text (or, inside a
    /// disclosure body, nested markdown).
    pub(super) fn html(&mut self, text: &str, range: Range<usize>) {
        self.end = self.end.max(range.end);
        for segment in html_segments(text) {
            match segment {
                HtmlSegment::Raw(raw) => match self.frames.last_mut() {
                    Some(frame) => match &mut frame.summary_raw {
                        Some(summary) => summary.push_str(raw),
                        None => frame.children.extend(parse_nested(raw)),
                    },
                    None => {
                        let runs = html_runs(raw);
                        if !runs.is_empty() {
                            self.out.push((Block::Paragraph { runs }, range.clone()));
                        }
                    }
                },
                HtmlSegment::DetailsOpen => {
                    self.finish_summary();
                    self.frames.push(DetailsFrame {
                        start: range.start,
                        summary: None,
                        summary_raw: None,
                        children: Vec::new(),
                    })
                }
                HtmlSegment::SummaryOpen => {
                    if let Some(frame) = self.frames.last_mut()
                        && frame.summary.is_none()
                        && frame.summary_raw.is_none()
                    {
                        frame.summary_raw = Some(String::new());
                    }
                }
                HtmlSegment::SummaryClose => self.finish_summary(),
                HtmlSegment::DetailsClose => self.close(range.end),
            }
        }
    }

    fn close(&mut self, end: usize) {
        self.finish_summary();
        let Some(frame) = self.frames.pop() else {
            return;
        };
        let block = Block::Details {
            summary: frame.summary.unwrap_or_default(),
            children: frame.children,
        };
        self.block(block, frame.start..end.max(frame.start));
    }

    /// `</details>` tags met inside inline content (a paragraph line that
    /// ends with the close tag) close that many frames.
    pub(super) fn close_inline(&mut self, count: usize, end: usize) {
        for _ in 0..count {
            if self.frames.is_empty() {
                break;
            }
            self.close(end);
        }
    }

    pub(super) fn finish(mut self) -> Vec<(Block, Range<usize>)> {
        let end = self.end;
        while !self.frames.is_empty() {
            self.close(end);
        }
        self.out
    }
}

/// The raw text of an HTML block (consumes through its `End`).
pub(super) fn html_block_text(cur: &mut Cursor) -> String {
    cur.bump();
    let mut text = String::new();
    loop {
        match cur.next_event() {
            Some(Event::Html(t)) | Some(Event::Text(t)) => text.push_str(&t),
            Some(Event::End(_)) | None => break,
            Some(_) => {}
        }
    }
    text
}

/// Markdown inside a disclosure body that arrived inside an HTML block.
/// Lines are dedented first: HTML indentation is layout, never code.
fn parse_nested(raw: &str) -> Vec<Block> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let dedented: String = raw
        .lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    parse_at(&dedented, 0)
        .blocks
        .into_iter()
        .map(|top| Arc::try_unwrap(top).map_or_else(|top| top.block.clone(), |top| top.block))
        .collect()
}

/// A `<summary>`'s label: its text parsed as inline markdown (agents write
/// both `<summary><b>x</b></summary>` and `<summary>**x**</summary>`).
fn summary_runs(raw: &str) -> Vec<InlineRun> {
    let mut runs: Vec<InlineRun> = Vec::new();
    fn collect(block: &Block, runs: &mut Vec<InlineRun>) {
        match block {
            Block::Paragraph { runs: inner } | Block::Heading { runs: inner, .. } => {
                if !runs.is_empty() {
                    runs.push(InlineRun {
                        text: " ".into(),
                        style: InlineStyle::default(),
                    });
                }
                runs.extend(inner.iter().cloned());
            }
            Block::BlockQuote { children, .. }
            | Block::Details { children, .. }
            | Block::FootnoteDefinition { children, .. } => {
                children.iter().for_each(|child| collect(child, runs))
            }
            Block::List { items, .. } => items.iter().flatten().for_each(|b| collect(b, runs)),
            Block::CodeBlock { code, .. } => runs.push(InlineRun {
                text: code.clone(),
                style: InlineStyle::default(),
            }),
            Block::Table { .. } | Block::Rule => {}
        }
    }
    for block in parse_nested(&raw.replace('\n', " ")) {
        collect(&block, &mut runs);
    }
    // Hard breaks would stack the label; a summary is one line.
    for run in &mut runs {
        if run.text.contains('\n') {
            run.text = run.text.replace('\n', " ");
        }
    }
    merge_runs(runs)
}

enum HtmlSegment<'a> {
    Raw(&'a str),
    DetailsOpen,
    DetailsClose,
    SummaryOpen,
    SummaryClose,
}

/// Split an HTML block into disclosure tags and the raw text between them.
/// Comments (closed or still streaming) are dropped here.
fn html_segments(text: &str) -> Vec<HtmlSegment<'_>> {
    let mut segments = Vec::new();
    let mut raw_start = 0;
    let mut at = 0;
    while let Some(rel) = text[at..].find('<') {
        let open = at + rel;
        let Some((token, len)) = parse_html_tag(&text[open..]) else {
            at = open + 1;
            continue;
        };
        let segment = match &token {
            HtmlToken::Comment => Some(None),
            HtmlToken::Tag { name, closing, .. } => match (name.as_str(), closing) {
                ("details", false) => Some(Some(HtmlSegment::DetailsOpen)),
                ("details", true) => Some(Some(HtmlSegment::DetailsClose)),
                ("summary", false) => Some(Some(HtmlSegment::SummaryOpen)),
                ("summary", true) => Some(Some(HtmlSegment::SummaryClose)),
                _ => None,
            },
            HtmlToken::Other => None,
        };
        if let Some(segment) = segment {
            if raw_start < open {
                segments.push(HtmlSegment::Raw(&text[raw_start..open]));
            }
            segments.extend(segment);
            raw_start = open + len;
        }
        at = open + len;
    }
    if raw_start < text.len() {
        segments.push(HtmlSegment::Raw(&text[raw_start..]));
    }
    segments
}

#[derive(Debug, PartialEq)]
pub(super) enum HtmlToken {
    Tag {
        name: String,
        closing: bool,
        self_closing: bool,
        attrs: String,
    },
    /// `<!-- … -->`, including one whose close has not streamed in yet.
    Comment,
    /// Declarations, processing instructions, CDATA.
    Other,
}

/// Parse one HTML tag at the start of `text`: the token and its byte length.
pub(super) fn parse_html_tag(text: &str) -> Option<(HtmlToken, usize)> {
    if let Some(rest) = text.strip_prefix("<!--") {
        let len = rest.find("-->").map_or(text.len(), |end| 4 + end + 3);
        return Some((HtmlToken::Comment, len));
    }
    if text.starts_with("<!") || text.starts_with("<?") {
        return text.find('>').map(|end| (HtmlToken::Other, end + 1));
    }
    let bytes = text.as_bytes();
    let mut at = 1;
    let closing = bytes.get(at) == Some(&b'/');
    if closing {
        at += 1;
    }
    let name_start = at;
    if !bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    while bytes
        .get(at)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        at += 1;
    }
    let name = text[name_start..at].to_ascii_lowercase();
    let attrs_start = at;
    let mut quote: Option<u8> = None;
    while let Some(&byte) = bytes.get(at) {
        match (quote, byte) {
            (Some(q), b) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'>') => {
                let attrs = text[attrs_start..at].trim();
                let self_closing = attrs.ends_with('/');
                let attrs = attrs.trim_end_matches('/').trim().to_string();
                return Some((
                    HtmlToken::Tag {
                        name,
                        closing,
                        self_closing,
                        attrs,
                    },
                    at + 1,
                ));
            }
            (None, b'<') => return None,
            _ => {}
        }
        at += 1;
    }
    None
}

/// A styling effect an inline HTML tag applies until its close.
#[derive(Debug, Clone)]
enum HtmlEffect {
    Bold,
    Italic,
    Code,
    Strike,
    Link(String),
}

/// Inline HTML state of one inline container: open styling tags in order.
/// Unknown tags never open anything — their text stays, the tags vanish.
#[derive(Default)]
pub(super) struct HtmlInline {
    open: Vec<(String, HtmlEffect)>,
}

/// Separator a block-level tag leaves in flattened HTML text (resolved to a
/// single line break by [`clean_html_runs`]).
const HTML_BLOCK_BREAK: &str = "\u{1}";

impl HtmlInline {
    pub(super) fn style(&self, base: &InlineStyle) -> InlineStyle {
        let mut style = base.clone();
        for (_, effect) in &self.open {
            match effect {
                HtmlEffect::Bold => style.bold = true,
                HtmlEffect::Italic => style.italic = true,
                HtmlEffect::Code => style.code = true,
                HtmlEffect::Strike => style.strikethrough = true,
                HtmlEffect::Link(url) => style.link = Some(url.clone()),
            }
        }
        style
    }

    /// Apply one tag; returns the text it stands for (`<br>` is a hard
    /// break; block tags separate lines when `block` is set).
    pub(super) fn apply(&mut self, token: &HtmlToken, block: bool) -> Option<&'static str> {
        let HtmlToken::Tag {
            name,
            closing,
            self_closing,
            attrs,
        } = token
        else {
            return None;
        };
        if name == "br" {
            return (!closing).then_some("\n");
        }
        let block_tag = block
            && matches!(
                name.as_str(),
                "p" | "div"
                    | "li"
                    | "tr"
                    | "ul"
                    | "ol"
                    | "table"
                    | "blockquote"
                    | "pre"
                    | "section"
                    | "center"
                    | "hr"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "dd"
                    | "dt"
            );
        let separator = block_tag.then_some(HTML_BLOCK_BREAK);
        if *closing {
            if let Some(pos) = self.open.iter().rposition(|(open, _)| open == name) {
                self.open.truncate(pos);
            }
            return separator;
        }
        if *self_closing {
            return separator;
        }
        let effect = match name.as_str() {
            "b" | "strong" => Some(HtmlEffect::Bold),
            "i" | "em" | "cite" | "var" | "dfn" => Some(HtmlEffect::Italic),
            "code" | "kbd" | "samp" | "tt" => Some(HtmlEffect::Code),
            "s" | "del" | "strike" => Some(HtmlEffect::Strike),
            "a" => html_href(attrs).map(HtmlEffect::Link),
            _ => None,
        };
        if let Some(effect) = effect {
            self.open.push((name.clone(), effect));
        }
        separator
    }
}

/// The `href` of an anchor tag when it is a web or mail link.
fn html_href(attrs: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let at = lower.find("href")?;
    let rest = attrs[at + 4..].trim_start().strip_prefix('=')?.trim_start();
    let value = match rest.as_bytes().first()? {
        quote @ (b'"' | b'\'') => {
            let inner = &rest[1..];
            &inner[..inner.find(*quote as char)?]
        }
        _ => rest.split_whitespace().next()?,
    };
    let scheme = value.get(..8).unwrap_or(value).to_ascii_lowercase();
    (scheme.starts_with("https://")
        || scheme.starts_with("http://")
        || scheme.starts_with("mailto:"))
    .then(|| value.to_string())
}

/// Styled text for a raw HTML block outside any disclosure: tags become
/// styling or vanish, comments drop, whitespace collapses as a browser
/// would, and `<br>` / block tags break lines.
fn html_runs(raw: &str) -> Vec<InlineRun> {
    let mut state = HtmlInline::default();
    let mut runs: Vec<InlineRun> = Vec::new();
    let push_text = |runs: &mut Vec<InlineRun>, text: &str, style: InlineStyle| {
        let decoded = decode_entities(text);
        let mut collapsed = String::with_capacity(decoded.len());
        let mut space = false;
        for ch in decoded.chars() {
            if ch.is_whitespace() && ch != '\u{a0}' {
                space = true;
            } else {
                if space {
                    collapsed.push(' ');
                    space = false;
                }
                collapsed.push(ch);
            }
        }
        if space {
            collapsed.push(' ');
        }
        if !collapsed.is_empty() {
            runs.push(InlineRun {
                text: collapsed,
                style,
            });
        }
    };
    let mut at = 0;
    let mut text_start = 0;
    while let Some(rel) = raw[at..].find('<') {
        let open = at + rel;
        let Some((token, len)) = parse_html_tag(&raw[open..]) else {
            at = open + 1;
            continue;
        };
        push_text(
            &mut runs,
            &raw[text_start..open],
            state.style(&InlineStyle::default()),
        );
        if let Some(text) = state.apply(&token, true) {
            runs.push(InlineRun {
                text: text.into(),
                style: InlineStyle::default(),
            });
        }
        at = open + len;
        text_start = at;
    }
    push_text(
        &mut runs,
        &raw[text_start..],
        state.style(&InlineStyle::default()),
    );
    autolink_runs(merge_runs(clean_html_runs(runs)))
}

/// Resolve collapsed HTML whitespace across runs: no spaces at line starts
/// or ends, block separators become one line break (never leading or
/// trailing), explicit `<br>` breaks stay as written.
fn clean_html_runs(runs: Vec<InlineRun>) -> Vec<InlineRun> {
    let mut out: Vec<InlineRun> = Vec::with_capacity(runs.len());
    // The last character emitted so far, across runs.
    let mut last: Option<char> = None;
    let mut pending_break = false;
    for run in runs {
        let mut text = String::with_capacity(run.text.len());
        for ch in run.text.chars() {
            match ch {
                '\u{1}' => pending_break = last.is_some() && last != Some('\n'),
                ' ' if last.is_none() || last == Some(' ') || last == Some('\n') => {}
                ' ' if pending_break => {}
                _ => {
                    if ch == '\n' {
                        trim_trailing_space(&mut out, &mut text);
                    }
                    if pending_break {
                        trim_trailing_space(&mut out, &mut text);
                        text.push('\n');
                        pending_break = false;
                    }
                    text.push(ch);
                    last = Some(ch);
                }
            }
        }
        if !text.is_empty() {
            out.push(InlineRun {
                text,
                style: run.style,
            });
        }
    }
    // Trailing whitespace and breaks carry nothing.
    while let Some(run) = out.last_mut() {
        let trimmed = run.text.trim_end_matches([' ', '\n']).len();
        run.text.truncate(trimmed);
        if run.text.is_empty() {
            out.pop();
        } else {
            break;
        }
    }
    out
}

fn trim_trailing_space(out: &mut [InlineRun], text: &mut String) {
    if text.is_empty() {
        if let Some(prev) = out.last_mut() {
            while prev.text.ends_with(' ') {
                prev.text.pop();
            }
        }
    } else {
        while text.ends_with(' ') {
            text.pop();
        }
    }
}

/// Decode the character references raw HTML text commonly carries
/// (pulldown-cmark decodes them in markdown text, never in raw HTML).
fn decode_entities(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let decoded = tail
            .as_bytes()
            .iter()
            .take(11)
            .position(|&byte| byte == b';')
            .and_then(|end| {
                let name = &tail[1..end];
                let ch = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    _ => name
                        .strip_prefix("#x")
                        .or_else(|| name.strip_prefix("#X"))
                        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                        .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                        .and_then(char::from_u32),
                }?;
                Some((ch, end + 1))
            });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::decode_entities;

    #[test]
    fn entity_decoding_preserves_unknown_and_unterminated_text() {
        let text = "é &unknown; &unfinished &&amp; &#x1F680; &#0000000065;";
        assert_eq!(
            decode_entities(text),
            "é &unknown; &unfinished && 🚀 &#0000000065;"
        );
    }
}
