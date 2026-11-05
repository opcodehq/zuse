//! Text-editing conveniences of the message composer that are pure over the
//! draft text: the kill ring behind Ctrl+K/Ctrl+U/word deletes, the
//! double-space period, `:shortcode:` emoji, the prompt stash and the
//! sent-message search. The composer wires them to keys and popups.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Kill ring
// ---------------------------------------------------------------------------

const MAX_KILLS: usize = 10;

/// Text removed by line and word deletions, newest first, shared by every
/// composer so Ctrl+Y pastes it back and Alt+Y cycles to older deletions
/// right after a paste.
#[derive(Debug, Default)]
pub(crate) struct KillRing {
    kills: VecDeque<String>,
    /// The span the last yank inserted, at which ring index, and the editor
    /// revision right after it. Anything else in between ends the yank.
    last_yank: Option<Yank>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Yank {
    pub owner: u64,
    pub start: usize,
    pub end: usize,
    pub index: usize,
    pub revision: u64,
}

impl gpui::Global for KillRing {}

impl KillRing {
    pub(crate) fn kill(&mut self, text: &str) {
        self.last_yank = None;
        if text.is_empty() {
            return;
        }
        self.kills.push_front(text.to_owned());
        self.kills.truncate(MAX_KILLS);
    }

    /// The newest deletion, if any.
    pub(crate) fn yank(&self) -> Option<(usize, &str)> {
        self.kills.front().map(|text| (0, text.as_str()))
    }

    /// The next older deletion to swap in for the text the previous yank
    /// inserted, when the editor is still exactly as that yank left it.
    /// Returns the previous yank's span, the ring index to record next and
    /// its text.
    pub(crate) fn yank_pop(&self, owner: u64, revision: u64) -> Option<(Yank, usize, &str)> {
        let last = self.last_yank.clone()?;
        if last.owner != owner || last.revision != revision || self.kills.len() < 2 {
            return None;
        }
        let index = (last.index + 1) % self.kills.len();
        Some((last, index, self.kills[index].as_str()))
    }

    pub(crate) fn record_yank(&mut self, yank: Yank) {
        self.last_yank = Some(yank);
    }
}

// ---------------------------------------------------------------------------
// Double space → period
// ---------------------------------------------------------------------------

/// Whether a space just typed at `pos - 1` completes a double space after a
/// word, so the two spaces before `pos` should become ". ". Only after a
/// letter, digit or closing bracket/quote, matching native text fields:
/// runs of spaces and existing punctuation are left alone.
pub(crate) fn completes_double_space_period(text: &str, pos: usize) -> bool {
    let Some(before) = text.get(..pos) else {
        return false;
    };
    let Some(rest) = before.strip_suffix("  ") else {
        return false;
    };
    rest.chars().next_back().is_some_and(|ch| {
        ch.is_alphanumeric() || matches!(ch, ')' | ']' | '}' | '"' | '\'' | '’' | '”')
    })
}

// ---------------------------------------------------------------------------
// Emoji shortcodes
// ---------------------------------------------------------------------------

/// Suggestions shown for a `:query`.
pub(crate) const EMOJI_SUGGESTION_LIMIT: usize = 8;
/// Longest shortcode name worth scanning back for.
const MAX_SHORTCODE_LEN: usize = 40;
/// The suggestion list opens once this many name characters follow the `:`.
const MIN_EMOJI_QUERY_LEN: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmojiSuggestion {
    pub emoji: &'static str,
    /// Shortcode name without colons, e.g. `heart`.
    pub name: &'static str,
}

pub(crate) struct EmojiIndex {
    by_name: HashMap<&'static str, &'static str>,
    /// One entry per alias, in data order.
    entries: Vec<EmojiSuggestion>,
}

/// GitHub's gemoji table (MIT), reduced to `emoji<TAB>alias alias…` lines.
const EMOJI_DATA: &str = include_str!("emoji.tsv");

/// Built on first use so the table costs nothing until someone types `:`.
pub(crate) fn emoji_index() -> &'static EmojiIndex {
    static INDEX: OnceLock<EmojiIndex> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut by_name = HashMap::new();
        let mut entries = Vec::new();
        for line in EMOJI_DATA.lines() {
            let Some((emoji, names)) = line.split_once('\t') else {
                continue;
            };
            for name in names.split(' ').filter(|name| !name.is_empty()) {
                if by_name.insert(name, emoji).is_none() {
                    entries.push(EmojiSuggestion { emoji, name });
                }
            }
        }
        EmojiIndex { by_name, entries }
    })
}

impl EmojiIndex {
    pub(crate) fn get(&self, name: &str) -> Option<&'static str> {
        self.by_name
            .get(name.to_ascii_lowercase().as_str())
            .copied()
    }

    /// Prefix matches first, shortest name first (so `hea` ranks `heart`
    /// above `heartbeat`), then names containing the query. Each emoji
    /// appears once even when several of its aliases match.
    pub(crate) fn suggestions(&self, query: &str, limit: usize) -> Vec<EmojiSuggestion> {
        let needle = query.to_ascii_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut prefixed: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.name.starts_with(&needle))
            .collect();
        prefixed.sort_by(|a, b| a.name.len().cmp(&b.name.len()).then(a.name.cmp(b.name)));
        let contained = self
            .entries
            .iter()
            .filter(|entry| !entry.name.starts_with(&needle) && entry.name.contains(&needle));
        let mut out: Vec<EmojiSuggestion> = Vec::new();
        for entry in prefixed.into_iter().chain(contained) {
            if out.len() >= limit {
                break;
            }
            if !out.iter().any(|seen| seen.emoji == entry.emoji) {
                out.push(entry.clone());
            }
        }
        out
    }
}

fn shortcode_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '+' | '-')
}

/// The `:name` run ending at `cursor`: the byte offset of its `:` and the
/// name. `None` for text that is not a shortcode candidate (`10:30`,
/// `https://`, `a:b`) or that sits in code.
fn scan_shortcode(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = text.get(..cursor)?;
    let name_len = before
        .chars()
        .rev()
        .take_while(|ch| shortcode_char(*ch))
        .count();
    if name_len > MAX_SHORTCODE_LEN {
        return None;
    }
    let start = cursor - name_len;
    let colon = start.checked_sub(1)?;
    if before.as_bytes()[colon] != b':' {
        return None;
    }
    let opener_ok = before[..colon]
        .chars()
        .next_back()
        .is_none_or(|ch| ch.is_whitespace() || matches!(ch, '(' | '[' | '{'));
    if !opener_ok || crate::composer_markdown::in_code(text, colon) {
        return None;
    }
    Some((colon, &before[start..]))
}

/// An open `:query` at the caret that should list suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmojiTrigger {
    /// The `:` through the caret.
    pub range: std::ops::Range<usize>,
    pub query: String,
}

pub(crate) fn emoji_trigger(text: &str, cursor: usize) -> Option<EmojiTrigger> {
    let (from, name) = scan_shortcode(text, cursor)?;
    (name.chars().count() >= MIN_EMOJI_QUERY_LEN).then(|| EmojiTrigger {
        range: from..cursor,
        query: name.to_owned(),
    })
}

/// A `:name:` whose closing colon was just typed at `colon`: the range to
/// replace (both colons included) and its emoji.
pub(crate) fn complete_shortcode(
    text: &str,
    colon: usize,
) -> Option<(std::ops::Range<usize>, &'static str)> {
    if text.as_bytes().get(colon) != Some(&b':') {
        return None;
    }
    let (from, name) = scan_shortcode(text, colon)?;
    if name.is_empty() {
        return None;
    }
    let emoji = emoji_index().get(name)?;
    Some((from..colon + 1, emoji))
}

// ---------------------------------------------------------------------------
// Prompt stash
// ---------------------------------------------------------------------------

pub(crate) const MAX_STASHED_PROMPTS: usize = 20;

/// A draft set aside with Ctrl+S. Persisted in the device settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StashedPrompt {
    pub id: String,
    pub text: String,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// Newest first, capped at [`MAX_STASHED_PROMPTS`].
pub(crate) fn stash_prompt(stash: &mut Vec<StashedPrompt>, prompt: StashedPrompt) {
    stash.insert(0, prompt);
    stash.truncate(MAX_STASHED_PROMPTS);
}

/// A restored prompt goes after whatever is already in the composer.
pub(crate) fn append_restored(current: &str, restored: &str) -> String {
    let kept = current.trim_end();
    if current.trim().is_empty() {
        restored.to_owned()
    } else {
        format!("{kept}\n\n{restored}")
    }
}

// ---------------------------------------------------------------------------
// Sent-message search
// ---------------------------------------------------------------------------

pub(crate) const MAX_SEARCH_RESULTS: usize = 50;

/// Something the composer can bring back: a sent message or a draft cleared
/// with Escape twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecallEntry {
    /// Unix milliseconds.
    pub at: i64,
    pub text: String,
}

/// Newest first with repeats and blank entries removed. `entries` are
/// oldest first.
pub(crate) fn unique_recall_entries(entries: &[RecallEntry]) -> Vec<RecallEntry> {
    let mut seen = std::collections::HashSet::new();
    entries
        .iter()
        .rev()
        .filter(|entry| {
            let key = one_line(&entry.text);
            !key.is_empty() && seen.insert(key)
        })
        .cloned()
        .collect()
}

/// Whitespace runs collapsed to single spaces, for list rows and matching.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut haystack = haystack.chars();
    needle.chars().all(|wanted| haystack.any(|ch| ch == wanted))
}

/// Entries (already newest first) matching `query`: every entry for a blank
/// query, else those containing it, then those holding its characters in
/// order. Case-insensitive.
pub(crate) fn search_recall<'a>(entries: &'a [RecallEntry], query: &str) -> Vec<&'a RecallEntry> {
    let needle = one_line(query).to_lowercase();
    if needle.is_empty() {
        return entries.iter().take(MAX_SEARCH_RESULTS).collect();
    }
    let haystacks: Vec<String> = entries
        .iter()
        .map(|entry| one_line(&entry.text).to_lowercase())
        .collect();
    let contains = (0..entries.len()).filter(|&ix| haystacks[ix].contains(&needle));
    let scattered = (0..entries.len())
        .filter(|&ix| !haystacks[ix].contains(&needle) && is_subsequence(&needle, &haystacks[ix]));
    contains
        .chain(scattered)
        .take(MAX_SEARCH_RESULTS)
        .map(|ix| &entries[ix])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_ring_yanks_the_newest_and_pops_only_right_after_a_yank() {
        let mut ring = KillRing::default();
        assert!(ring.yank().is_none());
        ring.kill("first");
        ring.kill("");
        ring.kill("second");
        assert_eq!(ring.yank(), Some((0, "second")));
        // No yank yet: nothing to cycle.
        assert!(ring.yank_pop(1, 7).is_none());
        ring.record_yank(Yank {
            owner: 1,
            start: 0,
            end: 6,
            index: 0,
            revision: 7,
        });
        let (last, index, older) = ring.yank_pop(1, 7).unwrap();
        assert_eq!((last.start, last.end, index, older), (0, 6, 1, "first"));
        assert!(ring.yank_pop(2, 7).is_none());
        // An edit since the yank ends the cycle.
        assert!(ring.yank_pop(1, 8).is_none());
        // A new kill ends it as well.
        ring.kill("third");
        assert!(ring.yank_pop(1, 7).is_none());
        for ix in 0..20 {
            ring.kill(&ix.to_string());
        }
        assert_eq!(ring.kills.len(), MAX_KILLS);
    }

    #[test]
    fn double_space_becomes_a_period_only_after_a_word() {
        assert!(completes_double_space_period("Hello  ", 7));
        assert!(completes_double_space_period("done)  ", 7));
        assert!(completes_double_space_period(
            "“quoted”  ",
            "“quoted”  ".len()
        ));
        assert!(completes_double_space_period("42  ", 4));
        assert!(!completes_double_space_period("Hello. ", 7));
        assert!(!completes_double_space_period("Hello.  ", 8));
        assert!(!completes_double_space_period("   ", 3));
        assert!(!completes_double_space_period("  ", 2));
        assert!(!completes_double_space_period("Hello ", 6));
    }

    #[test]
    fn shortcode_triggers_skip_times_urls_words_and_code() {
        assert_eq!(
            emoji_trigger("I :hea", 6),
            Some(EmojiTrigger {
                range: 2..6,
                query: "hea".into()
            })
        );
        assert_eq!(emoji_trigger(":+1", 3).unwrap().query, "+1");
        assert_eq!(emoji_trigger("(:sm", 4).unwrap().range, 1..4);
        // One character is not enough to open the list.
        assert_eq!(emoji_trigger(":h", 2), None);
        for text in ["10:30", "https://ex", "a:bc", "`x :hea", "```\n:hea"] {
            assert_eq!(emoji_trigger(text, text.len()), None, "{text}");
        }
    }

    #[test]
    fn a_closing_colon_completes_known_shortcodes() {
        let text = "love :heart:";
        assert_eq!(
            complete_shortcode(text, text.len() - 1),
            Some((5..12, "❤️"))
        );
        assert_eq!(complete_shortcode(":HEART:", 6), Some((0..7, "❤️")));
        assert_eq!(complete_shortcode(":nope_not_real:", 14), None);
        assert_eq!(complete_shortcode("::", 1), None);
        assert_eq!(complete_shortcode("10:30:", 5), None);
    }

    #[test]
    fn suggestions_rank_prefixes_by_length_and_list_each_emoji_once() {
        let index = emoji_index();
        let names: Vec<_> = index
            .suggestions("heart", EMOJI_SUGGESTION_LIMIT)
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names[0], "heart");
        assert!(names.len() <= EMOJI_SUGGESTION_LIMIT);
        let emojis: Vec<_> = index
            .suggestions("smile", EMOJI_SUGGESTION_LIMIT)
            .into_iter()
            .map(|entry| entry.emoji)
            .collect();
        let mut unique = emojis.clone();
        unique.dedup();
        assert_eq!(emojis.len(), unique.len());
        assert!(index.suggestions("", 8).is_empty());
        assert_eq!(index.get("thumbsup"), index.get("+1"));
    }

    #[test]
    fn the_stash_keeps_the_newest_twenty_and_restores_after_the_draft() {
        let mut stash = Vec::new();
        for ix in 0..25 {
            stash_prompt(
                &mut stash,
                StashedPrompt {
                    id: ix.to_string(),
                    text: format!("prompt {ix}"),
                    created_at: ix,
                },
            );
        }
        assert_eq!(stash.len(), MAX_STASHED_PROMPTS);
        assert_eq!(stash[0].id, "24");
        assert_eq!(append_restored("", "saved"), "saved");
        assert_eq!(append_restored("  \n", "saved"), "saved");
        assert_eq!(append_restored("draft \n", "saved"), "draft\n\nsaved");
        let json = serde_json::to_value(&stash[0]).unwrap();
        assert_eq!(json["createdAt"], 24);
    }

    #[test]
    fn recall_search_is_newest_first_unique_and_prefers_contiguous_matches() {
        let entries: Vec<_> = [
            "fix the login redirect",
            "refactor   the  router",
            "fix the login redirect",
            "   ",
            "write release notes",
        ]
        .into_iter()
        .enumerate()
        .map(|(ix, text)| RecallEntry {
            at: ix as i64,
            text: text.into(),
        })
        .collect();
        let unique = unique_recall_entries(&entries);
        let texts: Vec<_> = unique.iter().map(|entry| entry.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "write release notes",
                "fix the login redirect",
                "refactor   the  router"
            ]
        );
        let found: Vec<_> = search_recall(&unique, "the r")
            .into_iter()
            .map(|entry| entry.text.as_str())
            .collect();
        // Whitespace runs collapse before matching.
        assert_eq!(found, ["fix the login redirect", "refactor   the  router"]);
        // Scattered letters still match, after contiguous hits.
        let scattered: Vec<_> = search_recall(&unique, "wrn")
            .into_iter()
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(scattered, ["write release notes"]);
        assert_eq!(search_recall(&unique, "  ").len(), 3);
        assert!(search_recall(&unique, "zzz").is_empty());
    }
}
