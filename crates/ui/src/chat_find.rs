//! Find in chat: case-insensitive matching over the transcript's rows, and
//! the highlight state the markdown painter reads.
//!
//! The transcript is virtualized, so matching runs over the row model rather
//! than painted text. A match is "the Nth occurrence inside row R", which
//! survives streaming appends and rows scrolling in or out of view. Each
//! painted text element looks up its own occurrences when it paints.

use std::cell::RefCell;
use std::ops::Range;

use gpui::{Hsla, SharedString};

use crate::markdown::parser::{Block, InlineRun};

/// Longest query searched; protects against pasting whole files.
pub const QUERY_MAX_CHARS: usize = 200;

/// The trimmed, length-capped query, lowercased for matching.
pub fn normalize_query(query: &str) -> Vec<char> {
    query
        .trim()
        .chars()
        .take(QUERY_MAX_CHARS)
        .flat_map(char::to_lowercase)
        .collect()
}

/// Non-overlapping, case-insensitive occurrences of `needle` (already
/// lowercased by [`normalize_query`]) as byte ranges of `text`. Ranges stay
/// valid in the original text even where case folding changes length.
pub fn match_ranges(text: &str, needle: &[char]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if needle.is_empty() || text.is_empty() {
        return out;
    }
    let mut start = 0;
    while start < text.len() {
        match match_at(text, start, needle) {
            Some(end) => {
                out.push(start..end);
                start = end;
            }
            None => {
                start += text[start..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    out
}

fn match_at(text: &str, start: usize, needle: &[char]) -> Option<usize> {
    let mut matched = 0;
    for (offset, c) in text[start..].char_indices() {
        for lower in c.to_lowercase() {
            if needle.get(matched) != Some(&lower) {
                return None;
            }
            matched += 1;
        }
        if matched == needle.len() {
            return Some(start + offset + c.len_utf8());
        }
    }
    None
}

pub fn count_matches(texts: &[String], needle: &[char]) -> usize {
    texts
        .iter()
        .map(|text| match_ranges(text, needle).len())
        .sum()
}

fn runs_text(runs: &[InlineRun]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

/// The text elements a markdown block paints, in document order: one per
/// paragraph, heading or table cell, one per code line.
pub fn block_texts(block: &Block, out: &mut Vec<String>) {
    match block {
        Block::Paragraph { runs } | Block::Heading { runs, .. } => out.push(runs_text(runs)),
        Block::CodeBlock { code, .. } => out.extend(code.lines().map(str::to_owned)),
        Block::BlockQuote { children } => {
            for child in children {
                block_texts(child, out);
            }
        }
        Block::List { items, .. } => {
            for block in items.iter().flatten() {
                block_texts(block, out);
            }
        }
        Block::Table { header, rows, .. } => {
            for cell in header.iter().chain(rows.iter().flatten()) {
                out.push(runs_text(cell));
            }
        }
        Block::Rule => {}
    }
}

/// One occurrence: the `ordinal`-th match inside row `row_ix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindMatch {
    pub row_ix: usize,
    pub row_id: SharedString,
    pub ordinal: usize,
}

/// `index` wrapped into `0..total`.
pub fn wrap_index(index: isize, total: usize) -> Option<usize> {
    (total > 0).then(|| index.rem_euclid(total as isize) as usize)
}

/// Where a fresh query lands: the first match at or below `row_ix`, else the
/// first match overall.
pub fn first_index_from_row(row_ix: usize, matches: &[FindMatch]) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    Some(matches.iter().position(|m| m.row_ix >= row_ix).unwrap_or(0))
}

/// Keep the active match stable while the transcript changes underneath it:
/// follow the same (row, ordinal) when it still exists, otherwise stay as
/// close as possible to the previous position.
pub fn resolve_active(
    previous: Option<&FindMatch>,
    previous_index: Option<usize>,
    matches: &[FindMatch],
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if let Some(previous) = previous
        && let Some(same) = matches
            .iter()
            .position(|m| m.row_id == previous.row_id && m.ordinal == previous.ordinal)
    {
        return Some(same);
    }
    Some(previous_index.map_or(0, |ix| ix.min(matches.len() - 1)))
}

/// What the painter highlights this frame.
#[derive(Clone)]
pub struct Highlight {
    /// The painted transcript surface the find bar belongs to.
    pub surface: u64,
    pub needle: Vec<char>,
    /// The active match: its row id and occurrence inside that row.
    pub active: Option<(SharedString, usize)>,
    pub wash: Hsla,
    pub active_wash: Hsla,
}

impl Highlight {
    /// Whether a painted element key (`{row}:{ix}`, `{row}:u`,
    /// `{row}-code{c}-line{l}`) belongs to `row_id`.
    pub fn key_in_row(key: &str, row_id: &str) -> bool {
        key.strip_prefix(row_id)
            .is_some_and(|rest| rest.starts_with(':') || rest.starts_with("-code"))
    }
}

thread_local! {
    static HIGHLIGHT: RefCell<Option<Highlight>> = const { RefCell::new(None) };
}

/// Install (or replace) this frame's highlight for `highlight.surface`.
pub fn set_highlight(highlight: Highlight) {
    HIGHLIGHT.with(|h| *h.borrow_mut() = Some(highlight));
}

/// Drop the highlight when `surface` owns it.
pub fn clear_highlight(surface: u64) {
    HIGHLIGHT.with(|h| {
        let mut h = h.borrow_mut();
        if h.as_ref().is_some_and(|current| current.surface == surface) {
            *h = None;
        }
    });
}

pub fn current_highlight(surface: u64) -> Option<Highlight> {
    HIGHLIGHT.with(|h| {
        h.borrow()
            .as_ref()
            .filter(|current| current.surface == surface)
            .cloned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(rows: &[(usize, &str, usize)]) -> Vec<FindMatch> {
        rows.iter()
            .flat_map(|(row_ix, id, count)| {
                (0..*count).map(move |ordinal| FindMatch {
                    row_ix: *row_ix,
                    row_id: SharedString::from(id.to_string()),
                    ordinal,
                })
            })
            .collect()
    }

    #[test]
    fn matching_ignores_case_and_never_overlaps() {
        let needle = normalize_query("  AA ");
        assert_eq!(match_ranges("aAaa", &needle), vec![0..2, 2..4]);
        assert_eq!(
            match_ranges("Find FIND find", &normalize_query("find")).len(),
            3
        );
        assert!(match_ranges("anything", &normalize_query("   ")).is_empty());
    }

    #[test]
    fn match_offsets_stay_valid_in_the_original_text() {
        // "İ" lowercases to two chars; ranges must still slice the source.
        let text = "İstanbul and istanbul";
        let ranges = match_ranges(text, &normalize_query("stan"));
        assert_eq!(ranges.len(), 2);
        for range in ranges {
            assert_eq!(text[range].to_lowercase(), "stan");
        }
        let ranges = match_ranges("é café É", &normalize_query("é"));
        assert_eq!(ranges.len(), 3);
    }

    #[test]
    fn long_queries_are_capped() {
        let long = "x".repeat(QUERY_MAX_CHARS + 50);
        assert_eq!(normalize_query(&long).len(), QUERY_MAX_CHARS);
    }

    #[test]
    fn block_texts_follow_the_painted_elements() {
        let tree = crate::markdown::parser::parse_full(
            "Para **bold** text\n\n- one\n- two\n\n```\nline a\nline b\n```\n",
        );
        let mut texts = Vec::new();
        for block in &tree.blocks {
            block_texts(&block.block, &mut texts);
        }
        assert_eq!(
            texts,
            vec!["Para bold text", "one", "two", "line a", "line b"]
        );
        assert_eq!(count_matches(&texts, &normalize_query("line")), 2);
    }

    #[test]
    fn stepping_wraps_in_both_directions() {
        assert_eq!(wrap_index(3, 3), Some(0));
        assert_eq!(wrap_index(-1, 3), Some(2));
        assert_eq!(wrap_index(0, 0), None);
    }

    #[test]
    fn a_fresh_query_starts_at_the_viewport() {
        let all = matches(&[(1, "a", 2), (4, "b", 1), (9, "c", 1)]);
        assert_eq!(first_index_from_row(0, &all), Some(0));
        assert_eq!(first_index_from_row(3, &all), Some(2));
        assert_eq!(first_index_from_row(10, &all), Some(0));
        assert_eq!(first_index_from_row(0, &[]), None);
    }

    #[test]
    fn the_active_match_follows_its_row_while_the_chat_changes() {
        let before = matches(&[(1, "a", 2), (4, "b", 1)]);
        let active = before[2].clone();
        // A new row above shifts indices; the same match stays active.
        let after = matches(&[(0, "new", 1), (2, "a", 2), (5, "b", 1)]);
        assert_eq!(resolve_active(Some(&active), Some(2), &after), Some(3));
        // The match is gone: stay close to the old position.
        let shrunk = matches(&[(1, "a", 1)]);
        assert_eq!(resolve_active(Some(&active), Some(2), &shrunk), Some(0));
        assert_eq!(resolve_active(None, None, &[]), None);
    }

    #[test]
    fn element_keys_belong_to_their_row_only() {
        assert!(Highlight::key_in_row("m1#t0.3:0", "m1#t0.3"));
        assert!(Highlight::key_in_row("m1#u:u", "m1#u"));
        assert!(Highlight::key_in_row("m1#t0.3-code0-line2", "m1#t0.3"));
        assert!(!Highlight::key_in_row("m1#t0.31:0", "m1#t0.3"));
    }
}
