//! Mends half-streamed inline markdown for display (after streamdown's
//! `remend`, ported to zuse's incremental parser).
//!
//! While a block is streaming, an unclosed `**bold`, `*em`, `` `code ``,
//! `~~strike` or `[link](partial-url` parses as literal text; the closing
//! marker's arrival makes the marker characters vanish and restyles the run,
//! so wrap points shift and the paragraph's tail visibly reflows. Appending
//! synthetic closers to the *display* parse keeps styling stable from the
//! moment content follows an opener. Only the display tree sees mended text
//! ([`crate::parser::IncrementalParser::display_tree`]); the canonical tree is
//! untouched, so a marker that genuinely never closes settles honestly — one
//! flip when the row completes — instead of jittering throughout.
//!
//! Repairs, in the spirit of remend's handler set (its katex/html/comparison
//! handlers don't apply here — zuse renders raw HTML literally and has no
//! math):
//! - emphasis parity: `**a`→`**a**`, `*a`→`*a*`, `_a`/`__a`, `~~a`, nested
//!   closers innermost-first (`**a *b` → `**a *b***`), half-streamed closers
//!   completed (`**a*` → `**a**`);
//! - inline code backtick parity (fence-shaped tails never reach here — the
//!   parser gates mending on the last block not being a code block);
//! - links/images: the URL never renders — `[text](partial` and a bare
//!   `[text` both mend to `[text](`[`PENDING_LINK_URL`]`)`, so the link text
//!   shows styled immediately and the settling URL can't collapse the line;
//! - a last line of just `-`/`--`/`=`/`==` after text gets a zero-width space
//!   so a streaming list item is never misread as a setext underline (the
//!   paragraph-flashes-to-heading flicker).
//!
//! The scanner is deliberately approximate (streamdown ships the same
//! trade-off): it prefers "stable and close to the final parse" over exact
//! CommonMark delimiter resolution, because any mid-stream misjudgment is
//! repaired by the very next append or by the settle. Known accepted quirks:
//! intraword `**` closes (`2**3` briefly bolds the `3`), and whitespace-only
//! or marker-only content after an opener leaves the opener literal for a
//! chunk until real content arrives.
//!
//! Cost: one pass over the text plus the returned allocation, and `None`
//! (zero further work) whenever nothing hangs. Callers feed it only the last
//! top-level block of the stream, so per-append work stays O(tail) — the same
//! bound as the incremental reparse itself. remend re-scans the entire
//! accumulated message every delta; feeding just the tail is what makes the
//! port fit zuse's parser.

/// Sentinel destination for a link whose URL is still streaming. The renderer
/// styles it like any link but must not register it as clickable.
pub const PENDING_LINK_URL: &str = "zuse:pending-link";

/// One unclosed emphasis-family delimiter run (`*`, `_`, or `~~`).
struct OpenDelim {
    ch: char,
    len: usize,
    /// Char index just past the run: nesting order for closers, and the
    /// content-must-follow guard.
    pos: usize,
}

/// Repair hanging inline markers in a streaming block's source. Returns
/// `None` when the text needs no repair (the overwhelmingly common case).
pub fn close_hanging(text: &str) -> Option<String> {
    if let Some(mended) = alert_marker(text) {
        return Some(mended);
    }
    let cs: Vec<(usize, char)> = text.char_indices().collect();
    let n = cs.len();
    let at = |i: usize| cs.get(i).map(|&(_, c)| c);

    let mut delims: Vec<OpenDelim> = Vec::new();
    // Char indices of unmatched `[`.
    let mut brackets: Vec<usize> = Vec::new();
    // Open inline code span: (backtick run length, content char index).
    let mut code: Option<(usize, usize)> = None;
    // Char index of the last substantive character — content that justifies
    // closing an opener (not whitespace, not a bare marker).
    let mut last_content: Option<usize> = None;
    // Char index of the `]` of a `](…` whose URL runs off the end.
    let mut pending_url: Option<usize> = None;
    // Char index where a still-streaming HTML comment or tag begins: the
    // display drops it until it completes (a bare `<kb` never shows).
    let mut cut: Option<usize> = None;
    // An unclosed math span: (`$` run length, char index past the opener).
    let mut math: Option<(usize, usize)> = None;

    let mut math_closes = (1..n)
        .filter(|&j| {
            cs[j].1 == '$'
                && !cs[j - 1].1.is_whitespace()
                && cs[j - 1].1 != '\\'
                && at(j + 1) != Some('$')
        })
        .peekable();
    let mut last_tex = None;
    let mut i = 0;
    while i < n {
        let c = cs[i].1;
        if code.is_none() && c == '\\' {
            // Escaped char: both literal; the escapee still counts as content.
            if i + 1 < n {
                last_content = Some(i + 1);
            }
            i += 2;
            continue;
        }
        if c == '`' {
            let run = run_len(&cs, i);
            match code {
                // A span closes only on a run of the opening length.
                Some((open, _)) if run == open => code = None,
                Some(_) => last_content = Some(i + run - 1),
                None => code = Some((run, i + run)),
            }
            i += run;
            continue;
        }
        if code.is_some() {
            last_content = Some(i);
            i += 1;
            continue;
        }
        match c {
            '*' | '_' | '~' => {
                let run = run_len(&cs, i);
                delim(&mut delims, &cs, c, run, i, &mut last_content);
                i += run;
            }
            '[' => {
                brackets.push(i);
                i += 1;
            }
            ']' => {
                if let Some(open) = brackets.pop() {
                    // Emphasis opened inside a completed `[…]` and never
                    // closed there stays literal — as the final parse will
                    // decide too.
                    delims.retain(|d| d.pos < open);
                    if at(i + 1) == Some('(') {
                        // Consume the URL through its balanced `)`.
                        let mut j = i + 2;
                        let mut depth = 0usize;
                        loop {
                            match at(j) {
                                Some('(') => depth += 1,
                                Some(')') if depth == 0 => break,
                                Some(')') => depth -= 1,
                                Some(_) => {}
                                None => {
                                    pending_url = Some(i);
                                    break;
                                }
                            }
                            j += 1;
                        }
                        if pending_url.is_some() {
                            break;
                        }
                        last_content = Some(j);
                        i = j + 1;
                        continue;
                    }
                }
                last_content = Some(i);
                i += 1;
            }
            '<' => {
                let rest = &text[cs[i].0..];
                if let Some(len) = html_span_len(rest) {
                    // A whole tag or comment: attributes are not markdown.
                    let end = cs[i].0 + len;
                    last_content = Some(i);
                    while i < n && cs[i].0 < end {
                        i += 1;
                    }
                } else if partial_html_at_end(rest) {
                    cut = Some(i);
                    break;
                } else {
                    last_content = Some(i);
                    i += 1;
                }
            }
            '$' => {
                let run = run_len(&cs, i);
                if run >= 2 {
                    // Display math closes on the next `$$`; its body is TeX.
                    match (i + run..n.saturating_sub(1))
                        .find(|&j| cs[j].1 == '$' && cs[j + 1].1 == '$')
                    {
                        Some(j) => {
                            last_content = Some(j + 1);
                            i = j + 2;
                        }
                        None => {
                            math = Some((2, i + run));
                            break;
                        }
                    }
                    continue;
                }
                let next = at(i + 1);
                if next.is_some_and(|c| !c.is_whitespace()) {
                    // Inline math closes on a `$` not preceded by whitespace
                    // (pulldown-cmark's rule — `$5 and $10` stays text).
                    while math_closes.peek().is_some_and(|&j| j < i + 2) {
                        math_closes.next();
                    }
                    if let Some(j) = math_closes.next() {
                        last_content = Some(j);
                        i = j + 1;
                        continue;
                    }
                    // A hanging opener is math only when TeX follows; a
                    // price (`$5`) or a shell variable stays text.
                    if next.is_some_and(|c| !c.is_ascii_digit())
                        && last_tex
                            .get_or_insert_with(|| {
                                cs.iter().rposition(|&(_, c)| matches!(c, '\\' | '^' | '{'))
                            })
                            .is_some_and(|j| j > i)
                    {
                        math = Some((1, i + 1));
                        break;
                    }
                }
                last_content = Some(i);
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            _ => {
                last_content = Some(i);
                i += 1;
            }
        }
    }
    let text = match cut {
        Some(at) => &text[..cs[at].0],
        None => text,
    };

    // Text ends inside a link/image URL: drop the partial URL, keep the text.
    if let Some(close) = pending_url {
        let byte = cs[close].0;
        return Some(format!("{}]({PENDING_LINK_URL})", &text[..byte]));
    }

    // Collect closers innermost-first (descending open position). A `[` open
    // splits the emphasis closers naturally: delimiters opened inside the
    // link text close before the `](…)`, ones opened before it close after.
    let mut pending: Vec<(usize, String)> = Vec::new();
    if let Some((len, pos)) = math
        && cs[pos..].iter().any(|&(_, c)| !c.is_whitespace())
    {
        pending.push((pos, "$".repeat(len)));
    }
    if let Some((ticks, cpos)) = code
        && last_content.is_some_and(|lc| lc >= cpos)
    {
        pending.push((cpos, "`".repeat(ticks)));
    }
    for d in &delims {
        if last_content.is_some_and(|lc| lc >= d.pos) {
            pending.push((d.pos, d.ch.to_string().repeat(d.len)));
        }
    }
    if let Some(&open) = brackets.last()
        && last_content.is_some_and(|lc| lc > open)
    {
        pending.push((open, format!("]({PENDING_LINK_URL})")));
    }
    pending.sort_by_key(|b| std::cmp::Reverse(b.0));
    let closers: String = pending.into_iter().map(|(_, s)| s).collect();

    // A trailing line of only `-`/`--`/`=`/`==` under text is a setext
    // underline (or hr) to the parser but almost always a streaming list
    // item; a zero-width space breaks the reading invisibly until the next
    // characters decide. (Streamdown's setext handler, verbatim trick.)
    let setext = setext_partial(text);

    if closers.is_empty() && !setext {
        return cut.map(|_| text.to_string());
    }
    if setext {
        // Closers belong to the paragraph content above the underline line.
        let nl = text.rfind('\n');
        return Some(match (nl, closers.is_empty()) {
            (Some(nl), false) => {
                format!("{}{closers}{}\u{200B}", &text[..nl], &text[nl..])
            }
            _ => format!("{text}\u{200B}"),
        });
    }
    // Insert before trailing whitespace: a closer after a trailing space is
    // not right-flanking and would not close (remend trims the space for the
    // same reason).
    let end = text.trim_end().len();
    Some(format!("{}{closers}{}", &text[..end], &text[end..]))
}

/// The five GFM alert kinds, as their markers spell them.
const ALERT_KINDS: [&str; 5] = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"];

/// A quote whose only line is a half-typed alert marker (`> [!`, `> [!WAR`)
/// would show its marker as text, then snap into an alert. Complete the
/// marker when its letters already name one kind (every kind has its own
/// first letter), and hide a bare `[!` until the first letter decides.
fn alert_marker(text: &str) -> Option<String> {
    if text.contains('\n') {
        return None;
    }
    let trimmed = text.trim_start_matches(' ');
    if text.len() - trimmed.len() > 3 {
        return None;
    }
    let rest = trimmed.strip_prefix('>')?;
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    let letters = rest.strip_prefix("[!")?;
    if !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let quote = &text[..text.len() - rest.len()];
    if letters.is_empty() {
        return Some(quote.trim_end().to_string());
    }
    let upper = letters.to_ascii_uppercase();
    let mut kinds = ALERT_KINDS.iter().filter(|kind| kind.starts_with(&upper));
    let kind = kinds.next()?;
    Some(format!("{quote}[!{kind}]"))
}

/// Tags the display hides while their name is still streaming.
const KNOWN_TAGS: [&str; 41] = [
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "s",
    "samp",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "td",
    "th",
    "tr",
    "ul",
];

/// Byte length of a complete HTML tag or comment at the start of `text`.
fn html_span_len(text: &str) -> Option<usize> {
    if let Some(rest) = text.strip_prefix("<!--") {
        return rest.find("-->").map(|end| 4 + end + 3);
    }
    let bytes = text.as_bytes();
    let mut at = 1 + usize::from(bytes.get(1) == Some(&b'/'));
    if !bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    while bytes
        .get(at)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        at += 1;
    }
    match bytes.get(at) {
        Some(b'>') => return Some(at + 1),
        Some(b' ' | b'/' | b'\t') => {}
        _ => return None,
    }
    let close = text[at..].find(['>', '<', '\n'])?;
    (bytes[at + close] == b'>').then_some(at + close + 1)
}

/// An HTML comment or a known tag still streaming at the very end of the
/// block: `<!-- note`, `<det`, `</summ`, `<a href="x`.
fn partial_html_at_end(text: &str) -> bool {
    if text.starts_with("<!--") {
        return true; // `html_span_len` already found no close
    }
    if text.contains(['>', '\n']) {
        return false;
    }
    if matches!(text, "<!" | "<!-" | "</") {
        return true;
    }
    let body = text[1..].strip_prefix('/').unwrap_or(&text[1..]);
    let name_len = body
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(body.len());
    let (name, attrs) = body.split_at(name_len);
    if name.is_empty() {
        return false;
    }
    let name = name.to_ascii_lowercase();
    if attrs.is_empty() {
        KNOWN_TAGS.iter().any(|tag| tag.starts_with(&name))
    } else {
        attrs.starts_with([' ', '\t', '/']) && KNOWN_TAGS.contains(&name.as_str())
    }
}

fn run_len(cs: &[(usize, char)], i: usize) -> usize {
    let c = cs[i].1;
    cs[i..].iter().take_while(|&&(_, x)| x == c).count()
}

/// Match or open one delimiter run. Closes against the innermost same-char
/// opener (partially, for half-streamed closers like `**a*`); delimiters
/// opened after a consumed opener were inside the closed span and stay
/// literal, exactly as the final parse treats them.
fn delim(
    delims: &mut Vec<OpenDelim>,
    cs: &[(usize, char)],
    c: char,
    run: usize,
    i: usize,
    last_content: &mut Option<usize>,
) {
    let end = i + run;
    // GFM strikethrough is `~~` only; longer tilde runs are literal. A run of
    // one may still be the half-streamed first tilde of a closing `~~`, so it
    // proceeds to the matcher (but never opens).
    if c == '~' && run > 2 {
        *last_content = Some(end - 1);
        return;
    }
    let prev = i.checked_sub(1).map(|p| cs[p].1);
    let next = cs.get(end).map(|&(_, c)| c);
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    // Intraword `_` never delimits (CommonMark); intraword single `*` is
    // treated the same, conservatively — `2*3` must not flash italic.
    if word(prev) && word(next) && (c == '_' || (c == '*' && run == 1)) {
        *last_content = Some(end - 1);
        return;
    }
    let can_close = prev.is_some_and(|c| !c.is_whitespace());
    let can_open = next.is_some_and(|c| !c.is_whitespace());
    let mut rest = run;
    if can_close && let Some(k) = delims.iter().rposition(|d| d.ch == c) {
        let take = rest.min(delims[k].len);
        delims[k].len -= take;
        rest -= take;
        let keep = if delims[k].len == 0 { k } else { k + 1 };
        delims.truncate(keep);
    }
    if rest > 0 {
        // A lone `~` never opens; `~` entries of len 1 exist only as the
        // remainder of a half-streamed closer.
        if can_open && (c != '~' || rest == 2) {
            delims.push(OpenDelim {
                ch: c,
                len: rest,
                pos: end,
            });
        } else {
            *last_content = Some(end - 1);
        }
    }
}

/// Last line is only 1–2 `-` or `=` (no trailing whitespace) under a
/// non-empty line — the incomplete-setext ambiguity.
fn setext_partial(text: &str) -> bool {
    let Some(nl) = text.rfind('\n') else {
        return false;
    };
    let last = &text[nl + 1..];
    let trimmed = last.trim_start();
    let underline =
        |c: char| !trimmed.is_empty() && trimmed.len() <= 2 && trimmed.chars().all(|x| x == c);
    (underline('-') || underline('='))
        && text[..nl]
            .lines()
            .last()
            .is_some_and(|l| !l.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn mends(input: &str, expected: &str) {
        assert_eq!(close_hanging(input).as_deref(), Some(expected), "{input:?}");
    }

    #[track_caller]
    fn stays(input: &str) {
        assert_eq!(close_hanging(input), None, "{input:?}");
    }

    #[test]
    fn balanced_text_needs_nothing() {
        stays("plain words, no markers");
        stays("a **b** and *c* and `d` and ~~e~~");
        stays("[docs](https://x.dev) done");
        stays("");
    }

    #[test]
    fn bold_and_italic_close() {
        mends("**bold", "**bold**");
        mends("some *em", "some *em*");
        mends("a __b", "a __b__");
        mends("a _b", "a _b_");
        mends("***both", "***both***");
    }

    #[test]
    fn half_streamed_closers_complete() {
        mends("**bold*", "**bold**");
        mends("__b_", "__b__");
        mends("~~gone~", "~~gone~~");
    }

    #[test]
    fn nested_closers_come_innermost_first() {
        mends("**a *b", "**a *b***");
        mends("*a **b", "*a **b***");
        mends("_a **b", "_a **b**_");
    }

    #[test]
    fn bare_openers_stay_literal_until_content() {
        stays("**");
        stays("text **");
        stays("text ** ");
        stays("*");
        stays("~~");
        stays("`");
    }

    #[test]
    fn closers_insert_before_trailing_whitespace() {
        // `**bold **` would be left-flanking only — it must mend to
        // `**bold** ` to actually close.
        mends("**bold ", "**bold** ");
        mends("*em\n", "*em*\n");
    }

    #[test]
    fn intraword_and_escapes_are_literal() {
        stays("2*3 equals 6");
        stays("snake_case_name");
        stays("20~25 degrees");
        stays(r"\*not emphasis");
        stays(r"a \** b");
    }

    #[test]
    fn list_markers_are_not_openers() {
        stays("* item one");
        stays("- a\n* b");
    }

    #[test]
    fn strikethrough_closes() {
        mends("~~gone", "~~gone~~");
        stays("~single~x"); // run of 1 is literal
    }

    #[test]
    fn inline_code_closes_and_shields_markers() {
        mends("`code", "`code`");
        mends("call `a ** b", "call `a ** b`");
        mends("``a`", "``a```"); // shorter run is span content, not a closer
        stays("`done` after");
    }

    #[test]
    fn links_mend_to_pending_sentinel() {
        mends("[docs](https://x.dev/lo", "[docs](zuse:pending-link)");
        mends("[docs](", "[docs](zuse:pending-link)");
        mends("see [do", "see [do](zuse:pending-link)");
        mends("![alt](https://x/i.p", "![alt](zuse:pending-link)");
        stays("see [");
        stays("[x] task-like"); // completed bracket, no url
    }

    #[test]
    fn link_urls_allow_nested_parens() {
        stays("[a](https://x.dev/(y)) done");
        mends("[a](https://x.dev/(y", "[a](zuse:pending-link)");
    }

    #[test]
    fn emphasis_inside_link_text_closes_inside() {
        mends("[**a", "[**a**](zuse:pending-link)");
        mends("**a [b", "**a [b](zuse:pending-link)**");
    }

    #[test]
    fn emphasis_unclosed_in_completed_bracket_is_dropped() {
        stays("[**a] done");
    }

    #[test]
    fn setext_partials_get_zero_width_space() {
        mends("para\n-", "para\n-\u{200B}");
        mends("para\n--", "para\n--\u{200B}");
        mends("para\n=", "para\n=\u{200B}");
        stays("para\n---"); // real hr / settled setext
        stays("-"); // no line above
        stays("\n-"); // empty line above
        mends("**b\n-", "**b**\n-\u{200B}"); // closers go above the underline
    }
}
