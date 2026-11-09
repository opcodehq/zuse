//! Zot panel tabs shared by the native and browser clients.

/// Pages of a Zot's side panel. Only the active page shows its label; the
/// rest collapse to icons and expand when selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZotTab {
    #[default]
    Overview,
    Threads,
    Drive,
    History,
}

impl ZotTab {
    pub const ALL: [ZotTab; 4] = [Self::Overview, Self::Threads, Self::Drive, Self::History];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Threads => "Threads",
            Self::Drive => "Drive",
            Self::History => "History",
        }
    }

    /// Icon name shared by both clients' asset sets.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Overview => "widget",
            Self::Threads => "chat-round-line",
            Self::Drive => "folder-open",
            Self::History => "history",
        }
    }

    /// Width the expanded label grows to, from a per-character estimate at
    /// the 12px tab type size. Generous so the label never clips.
    pub fn label_width(self) -> f32 {
        self.label().chars().count() as f32 * 7.5 + 4.0
    }
}

/// Selection with the previous page kept so both the expanding and the
/// collapsing tab animate; `epoch` restarts the animation on every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ZotTabs {
    pub active: ZotTab,
    pub previous: Option<ZotTab>,
    pub epoch: u64,
}

impl ZotTabs {
    pub fn select(&mut self, tab: ZotTab) -> bool {
        if self.active == tab {
            return false;
        }
        self.previous = Some(self.active);
        self.active = tab;
        self.epoch += 1;
        true
    }

    /// Label reveal (0 hidden, 1 shown) for `tab` at animation progress `t`.
    pub fn reveal(&self, tab: ZotTab, t: f32) -> f32 {
        if tab == self.active {
            if self.epoch == 0 { 1.0 } else { t }
        } else if Some(tab) == self.previous {
            1.0 - t
        } else {
            0.0
        }
    }
}

/// Workers of a Zot belong under it, not in project lists.
pub fn zot_owns_chat(zots: &[crate::zots::Zot], chat_id: &str) -> bool {
    zots.iter().any(|z| {
        z.coordinator_chat_id == chat_id
            || z.previous_coordinator_chats.iter().any(|c| c == chat_id)
            || z.workers.iter().any(|w| w.chat_id == chat_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_animates_both_tabs() {
        let mut tabs = ZotTabs::default();
        assert_eq!(tabs.reveal(ZotTab::Overview, 0.0), 1.0);
        assert!(!tabs.select(ZotTab::Overview));
        assert!(tabs.select(ZotTab::Drive));
        assert_eq!(tabs.reveal(ZotTab::Drive, 0.25), 0.25);
        assert_eq!(tabs.reveal(ZotTab::Overview, 0.25), 0.75);
        assert_eq!(tabs.reveal(ZotTab::Threads, 0.5), 0.0);
        assert_eq!(tabs.epoch, 1);
    }
}
