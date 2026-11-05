//! Composer keyboard state that outlives a single keystroke: browsing the
//! chat's sent messages with Up/Down, and the two-press Escape that stops a
//! running agent.

use std::time::{Duration, Instant};

/// How long the second Escape may trail the first and still stop the agent.
pub(crate) const DOUBLE_ESCAPE_WINDOW: Duration = Duration::from_millis(500);

/// One cursor over the chat's sent messages. The list is captured when
/// browsing starts and stays fixed until the user edits, sends or navigates.
#[derive(Debug, Default)]
pub(crate) struct MessageRecall {
    /// Position in `entries` (newest first); `None` while showing the draft.
    index: Option<usize>,
    /// The unsent draft, restored after stepping past the newest message.
    draft: String,
    entries: Vec<String>,
    /// The text recall last put in the editor. Any other text is an edit.
    shown: Option<String>,
}

impl MessageRecall {
    pub(crate) fn browsing(&self) -> bool {
        self.index.is_some()
    }

    /// The draft to keep for this chat if the user leaves mid-browse.
    pub(crate) fn unsent_draft(&self) -> Option<&str> {
        self.browsing().then_some(self.draft.as_str())
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// An edit to a recalled message makes it the new draft.
    pub(crate) fn observe_text(&mut self, text: &str) {
        if self.shown.as_deref().is_some_and(|shown| shown != text) {
            self.reset();
        }
    }

    /// Step `-1` (older) or `1` (newer). `messages` are oldest first and are
    /// only read when browsing starts. Returns the text to show, or `None`
    /// when there is nothing further in that direction.
    pub(crate) fn navigate(
        &mut self,
        direction: isize,
        current: &str,
        messages: impl FnOnce() -> Vec<String>,
    ) -> Option<String> {
        if self.index.is_none() {
            if direction > 0 {
                return None;
            }
            let mut entries = messages();
            if entries.is_empty() {
                return None;
            }
            entries.reverse();
            self.entries = entries;
            self.draft = current.to_owned();
        }
        let position = self.index.map_or(-1, |index| index as isize) - direction;
        if position >= self.entries.len() as isize {
            return None;
        }
        let text = if position < 0 {
            self.index = None;
            std::mem::take(&mut self.draft)
        } else {
            self.index = Some(position as usize);
            self.entries[position as usize].clone()
        };
        self.shown = Some(text.clone());
        Some(text)
    }
}

/// Two deliberate Escape presses on the same draft revision. A held key
/// repeats without a release in between, so it never counts twice.
#[derive(Debug, Default)]
pub(crate) struct DoubleEscape {
    first: Option<(Instant, u64)>,
    released: bool,
}

impl DoubleEscape {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn release(&mut self) {
        self.released = self.first.is_some();
    }

    /// Record a press. True when it completes a double press.
    pub(crate) fn press(&mut self, revision: u64, now: Instant) -> bool {
        let completes = self.released
            && self.first.is_some_and(|(at, first_revision)| {
                first_revision == revision
                    && now
                        .checked_duration_since(at)
                        .is_some_and(|elapsed| elapsed <= DOUBLE_ESCAPE_WINDOW)
            });
        if completes {
            self.reset();
            return true;
        }
        self.first = Some((now, revision));
        self.released = false;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent() -> Vec<String> {
        vec!["first".into(), "second".into(), "third".into()]
    }

    #[test]
    fn up_walks_back_from_the_newest_message_and_down_restores_the_draft() {
        let mut recall = MessageRecall::default();
        assert_eq!(recall.navigate(-1, "draft", sent).as_deref(), Some("third"));
        assert!(recall.browsing());
        assert_eq!(recall.unsent_draft(), Some("draft"));
        assert_eq!(
            recall.navigate(-1, "third", Vec::new).as_deref(),
            Some("second")
        );
        assert_eq!(
            recall.navigate(-1, "second", Vec::new).as_deref(),
            Some("first")
        );
        // The oldest message is the end of the list.
        assert_eq!(recall.navigate(-1, "first", Vec::new), None);
        assert_eq!(
            recall.navigate(1, "first", Vec::new).as_deref(),
            Some("second")
        );
        assert_eq!(
            recall.navigate(1, "second", Vec::new).as_deref(),
            Some("third")
        );
        assert_eq!(
            recall.navigate(1, "third", Vec::new).as_deref(),
            Some("draft")
        );
        assert!(!recall.browsing());
        assert_eq!(recall.navigate(1, "draft", Vec::new), None);
    }

    #[test]
    fn down_without_browsing_and_an_empty_history_do_nothing() {
        let mut recall = MessageRecall::default();
        assert_eq!(recall.navigate(1, "draft", sent), None);
        assert_eq!(recall.navigate(-1, "draft", Vec::new), None);
        assert!(!recall.browsing());
        assert_eq!(recall.unsent_draft(), None);
    }

    #[test]
    fn the_list_is_captured_once_per_browse() {
        let mut recall = MessageRecall::default();
        recall.navigate(-1, "", sent);
        let mut read_again = false;
        recall.navigate(-1, "third", || {
            read_again = true;
            Vec::new()
        });
        assert!(!read_again);
    }

    #[test]
    fn editing_a_recalled_message_makes_it_the_draft() {
        let mut recall = MessageRecall::default();
        recall.navigate(-1, "draft", sent);
        // The editor echoing the recalled text is not an edit.
        recall.observe_text("third");
        assert!(recall.browsing());
        recall.observe_text("third!");
        assert!(!recall.browsing());
        assert_eq!(recall.unsent_draft(), None);
        // Browsing again starts from the edited text.
        assert_eq!(
            recall.navigate(-1, "third!", sent).as_deref(),
            Some("third")
        );
        assert_eq!(
            recall.navigate(1, "third", Vec::new).as_deref(),
            Some("third!")
        );
    }

    #[test]
    fn double_escape_needs_two_releases_within_the_window_on_one_revision() {
        let start = Instant::now();
        let mut presses = DoubleEscape::default();
        assert!(!presses.press(1, start));
        presses.release();
        assert!(presses.press(1, start + Duration::from_millis(500)));

        // A held key repeats without a release.
        let mut held = DoubleEscape::default();
        assert!(!held.press(1, start));
        assert!(!held.press(1, start + Duration::from_millis(30)));

        // Too slow: the second press starts a new pair.
        let mut slow = DoubleEscape::default();
        slow.press(1, start);
        slow.release();
        assert!(!slow.press(1, start + Duration::from_millis(501)));
        slow.release();
        assert!(slow.press(1, start + Duration::from_millis(700)));

        // Typing between the presses changes the revision.
        let mut edited = DoubleEscape::default();
        edited.press(1, start);
        edited.release();
        assert!(!edited.press(2, start + Duration::from_millis(100)));

        // A reset forgets the first press.
        let mut reset = DoubleEscape::default();
        reset.press(1, start);
        reset.release();
        reset.reset();
        assert!(!reset.press(1, start + Duration::from_millis(100)));
    }
}
