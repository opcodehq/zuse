//! Zapper panel tabs shared by the native and browser clients.

/// Pages of a Zapper's side panel. Only the active page shows its label; the
/// rest collapse to icons and expand when selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ZapperTab {
    #[default]
    Overview,
    Issues,
    Threads,
    Drive,
    History,
}

impl ZapperTab {
    pub const ALL: [ZapperTab; 5] = [
        Self::Overview,
        Self::Issues,
        Self::Threads,
        Self::Drive,
        Self::History,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Issues => "Issues",
            Self::Threads => "Threads",
            Self::Drive => "Drive",
            Self::History => "History",
        }
    }

    /// Icon name shared by both clients' asset sets.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Overview => "list",
            Self::Issues => "clock-circle",
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
pub struct ZapperTabs {
    pub active: ZapperTab,
    pub previous: Option<ZapperTab>,
    pub epoch: u64,
}

impl ZapperTabs {
    pub fn select(&mut self, tab: ZapperTab) -> bool {
        if self.active == tab {
            return false;
        }
        self.previous = Some(self.active);
        self.active = tab;
        self.epoch += 1;
        true
    }

    /// Label reveal (0 hidden, 1 shown) for `tab` at animation progress `t`.
    pub fn reveal(&self, tab: ZapperTab, t: f32) -> f32 {
        if tab == self.active {
            if self.epoch == 0 { 1.0 } else { t }
        } else if Some(tab) == self.previous {
            1.0 - t
        } else {
            0.0
        }
    }
}

/// Section pages that always sit at the head of the native panel's strip.
/// History is not one: it opens from the strip's overflow menu.
pub const ZAPPER_SECTIONS: [ZapperTab; 4] = [
    ZapperTab::Overview,
    ZapperTab::Issues,
    ZapperTab::Threads,
    ZapperTab::Drive,
];

/// One page of the native Zapper panel: a section, or a closable tab opened
/// from one (an issue, a Drive document, the version history) or from the
/// coordinator chat (a pull request, a subagent, a diff).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ZapperPage {
    Section(ZapperTab),
    /// An issue, by id.
    Issue(String),
    /// A Drive document, by path.
    File(String),
    /// A pull request, by URL: linked from an issue, or opened from the
    /// coordinator chat.
    PullRequest(String),
    History,
    /// A subagent's transcript opened from the coordinator chat, by the
    /// client's handle for it.
    Subagent(u64),
    /// A diff (the checkout review) opened from the coordinator chat, by the
    /// client's handle for it.
    Diff(u64),
}

impl Default for ZapperPage {
    fn default() -> Self {
        Self::Section(ZapperTab::Overview)
    }
}

impl ZapperPage {
    pub fn closable(&self) -> bool {
        !matches!(self, Self::Section(_))
    }
}

/// The native panel's tab strip: fixed sections followed by closable tabs,
/// like an editor. Closing a tab returns to the page shown before it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZapperPanelTabs {
    active: ZapperPage,
    /// The section last shown. It keeps its label while a closable tab is
    /// active, so the section group never changes width under the pointer.
    section: ZapperTab,
    /// Closable tabs, in strip order.
    open: Vec<ZapperPage>,
    /// Pages shown before the active one, most recent last.
    trail: Vec<ZapperPage>,
}

impl ZapperPanelTabs {
    pub fn active(&self) -> &ZapperPage {
        &self.active
    }

    /// The section last shown: the active one, or the one a closable tab
    /// was opened over. Closing tabs falls back to it.
    pub fn section(&self) -> ZapperTab {
        self.section
    }

    /// The section button that shows its label: only the active section.
    /// With an issue, document, pull request or History tab active, every
    /// section is a plain icon.
    pub fn expanded_section(&self) -> Option<ZapperTab> {
        match self.active {
            ZapperPage::Section(tab) => Some(tab),
            _ => None,
        }
    }

    pub fn open_tabs(&self) -> &[ZapperPage] {
        &self.open
    }

    pub fn is_active(&self, page: &ZapperPage) -> bool {
        &self.active == page
    }

    /// The issue the active tab shows, if any.
    pub fn active_issue(&self) -> Option<&str> {
        match &self.active {
            ZapperPage::Issue(id) => Some(id),
            _ => None,
        }
    }

    /// Show `page`, adding it to the strip when it is closable and not open
    /// yet. Returns whether anything changed.
    pub fn show(&mut self, page: ZapperPage) -> bool {
        if self.active == page {
            return false;
        }
        match &page {
            ZapperPage::Section(tab) => self.section = *tab,
            _ if !self.open.contains(&page) => self.open.push(page.clone()),
            _ => {}
        }
        let previous = std::mem::replace(&mut self.active, page);
        self.trail.retain(|p| p != &previous && p != &self.active);
        self.trail.push(previous);
        true
    }

    pub fn select(&mut self, tab: ZapperTab) -> bool {
        self.show(ZapperPage::Section(tab))
    }

    /// Remove a closable tab. Closing the active one returns to the page
    /// shown before it that is still in the strip.
    pub fn close(&mut self, page: &ZapperPage) -> bool {
        let Some(index) = self.open.iter().position(|p| p == page) else {
            return false;
        };
        self.open.remove(index);
        self.trail.retain(|p| p != page);
        if &self.active == page {
            self.active = self
                .trail
                .pop()
                .unwrap_or(ZapperPage::Section(self.section));
            if let ZapperPage::Section(tab) = self.active {
                self.section = tab;
            }
        }
        true
    }

    /// A section left the strip (Drive with no documents): show Overview in
    /// its place without disturbing an open tab.
    pub fn hide_section(&mut self, gone: ZapperTab) -> bool {
        let hidden = ZapperPage::Section(gone);
        self.trail.retain(|p| p != &hidden);
        if self.active == hidden {
            self.active = ZapperPage::Section(ZapperTab::Overview);
        }
        let changed = self.section == gone;
        if changed {
            self.section = ZapperTab::Overview;
        }
        changed
    }

    /// Back to Overview with no tabs, for a different Zapper.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Workers of a Zapper belong under it, not in project lists.
pub fn zapper_owns_chat(zappers: &[crate::zappers::Zapper], chat_id: &str) -> bool {
    zappers.iter().any(|z| {
        z.coordinator_chat_id == chat_id
            || z.previous_coordinator_chats.iter().any(|c| c == chat_id)
            || z.workers.iter().any(|w| w.chat_id == chat_id)
    })
}

/// Whether `chat_id` has the shape the engine gives Zapper conversations:
/// `zapper-<zapper>-<n>` for coordinators and `zapper-worker-<command>` for the
/// threads they start. Lets clients keep a brand-new thread out of project
/// lists before the Zapper's own list catches up. Ids minted before the
/// rename (`zot-…`) still count.
pub fn zapper_chat_id(chat_id: &str) -> bool {
    use crate::zappers::{COORDINATOR_CHAT_PREFIX, legacy};
    chat_id.starts_with(COORDINATOR_CHAT_PREFIX)
        || chat_id.starts_with(legacy::COORDINATOR_CHAT_PREFIX)
}

/// Whether `chat_id` is a thread a Zapper started: the engine names every
/// delegated thread `zapper-worker-<command>` and the id never changes, so this
/// still holds after the Zapper releases, stops or archives it.
pub fn zapper_thread_chat_id(chat_id: &str) -> bool {
    use crate::zappers::{WORKER_CHAT_PREFIX, legacy};
    chat_id.starts_with(WORKER_CHAT_PREFIX) || chat_id.starts_with(legacy::WORKER_CHAT_PREFIX)
}

/// Whether a chat stays out of project lists for good: a conversation a
/// Zapper owns right now (coordinator, thread, or a side chat of one), or a
/// thread a Zapper started, in any later state. Releasing a thread drops it
/// from the Zapper's `workers`, so ownership alone let released threads fall
/// back into their project; the `zapper-worker-` id keeps them out, including
/// threads released before this check existed. Threads a Zapper adopted from
/// an ordinary chat keep their own id and rejoin their project on release.
pub fn zapper_keeps_chat(
    zappers: &[crate::zappers::Zapper],
    chat_id: &str,
    parent_chat_id: Option<&str>,
    thread_id: Option<&str>,
) -> bool {
    std::iter::once(chat_id)
        .chain(parent_chat_id)
        .chain(thread_id)
        .any(|id| zapper_owns_chat(zappers, id) || zapper_thread_chat_id(id))
}

/// Threads a Zapper started that are no longer among its workers (released,
/// or dropped when it finished) but are still linked to one of its issues:
/// newest issue first, and within an issue its latest thread first. The Threads tab lists them under "Finished" so they stay
/// reachable from the Zapper once they leave the active list.
pub fn finished_zapper_threads(zapper: &crate::zappers::Zapper) -> Vec<String> {
    let mut finished: Vec<String> = Vec::new();
    for issue in zapper.issues.iter().rev() {
        for id in issue.threads.iter().rev() {
            if zapper_thread_chat_id(id)
                && !zapper.workers.iter().any(|w| &w.chat_id == id)
                && !finished.contains(id)
            {
                finished.push(id.clone());
            }
        }
    }
    finished
}

/// Where a thread sits in a Zapper's Threads lists. Working threads lead,
/// then ones waiting on the user, then queued, then idle or finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreadRank {
    Working,
    NeedsYou,
    Queued,
    Idle,
}

/// A Zapper's threads in the order its Overview and Threads tab list them:
/// by [`ThreadRank`], newest first within a rank. The engine appends each new
/// thread to `workers`, so newest first is their reverse. A queued thread
/// ranks as queued whatever `rank` says: it has no conversation yet.
pub fn ordered_zapper_threads<'a>(
    workers: impl DoubleEndedIterator<Item = &'a crate::zappers::ZapperWorker>,
    rank: impl Fn(&crate::zappers::ZapperWorker) -> ThreadRank,
) -> Vec<&'a crate::zappers::ZapperWorker> {
    let mut threads: Vec<_> = workers.rev().collect();
    threads.sort_by_key(|w| {
        if w.queued() {
            ThreadRank::Queued
        } else {
            rank(w)
        }
    });
    threads
}

/// One thing in the Overview's "Needs you" card: something that really waits
/// on the user. Coordinator events (thread reports, linked pull requests,
/// interruptions) are the Zapper's inbox and never appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeedsYouItem {
    /// An open decision with no answer yet.
    Decision { id: String, question: String },
    /// An issue the Zapper handed to the user.
    Issue {
        id: String,
        code: String,
        title: String,
        /// The latest question the Zapper asked on the issue, if any.
        question: Option<String>,
    },
}

/// What Open on a "Needs you" item shows. Never a chat: the items have no
/// chat of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeedsYouTarget {
    /// The issue's tab.
    IssueTab(String),
    /// The answer form of the decision.
    DecisionForm(String),
}

impl NeedsYouItem {
    pub fn target(&self) -> NeedsYouTarget {
        match self {
            Self::Decision { id, .. } => NeedsYouTarget::DecisionForm(id.clone()),
            Self::Issue { id, .. } => NeedsYouTarget::IssueTab(id.clone()),
        }
    }
    /// The row's title.
    pub fn title(&self) -> String {
        match self {
            Self::Decision { question, .. } => question.clone(),
            Self::Issue {
                code,
                title,
                question,
                ..
            } => format!("{code} · {}", question.as_deref().unwrap_or(title)),
        }
    }
}

/// What waits on the user in a Zapper's `issues` and `decisions`: open issues owned by the user (with the
/// latest `question` activity) first, then decisions with no answer. Both
/// leave the list once answered or handed back, because they are read from
/// the current owner and answer.
pub fn needs_you_items(
    issues: &[crate::zappers::ZapperIssue],
    decisions: &[crate::zappers::ZapperDecision],
) -> Vec<NeedsYouItem> {
    use crate::zappers::{ZapperIssueOwner, ZapperIssueStatus};
    let issues = issues
        .iter()
        .filter(|i| {
            i.details.status == ZapperIssueStatus::NeedsYou
                && i.details.current_owner() == Some(ZapperIssueOwner::User)
        })
        .map(|i| NeedsYouItem::Issue {
            id: i.id.clone(),
            code: i.code(),
            title: i.details.title.clone(),
            question: i
                .activity
                .iter()
                .rev()
                .find(|a| a.kind == "question" && !a.text.trim().is_empty())
                .map(|a| a.text.trim().to_owned()),
        });
    let decisions =
        decisions
            .iter()
            .filter(|d| d.answer.is_none())
            .map(|d| NeedsYouItem::Decision {
                id: d.id.clone(),
                question: d.question.clone(),
            });
    issues.chain(decisions).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_animates_both_tabs() {
        let mut tabs = ZapperTabs::default();
        assert_eq!(tabs.reveal(ZapperTab::Overview, 0.0), 1.0);
        assert!(!tabs.select(ZapperTab::Overview));
        assert!(tabs.select(ZapperTab::Drive));
        assert_eq!(tabs.reveal(ZapperTab::Drive, 0.25), 0.25);
        assert_eq!(tabs.reveal(ZapperTab::Overview, 0.25), 0.75);
        assert_eq!(tabs.reveal(ZapperTab::Threads, 0.5), 0.0);
        assert_eq!(tabs.epoch, 1);
    }

    fn issue(id: &str) -> ZapperPage {
        ZapperPage::Issue(id.into())
    }

    #[test]
    fn issues_and_files_open_as_tabs_after_the_sections() {
        let mut tabs = ZapperPanelTabs::default();
        assert_eq!(tabs.active(), &ZapperPage::Section(ZapperTab::Overview));
        assert!(tabs.select(ZapperTab::Issues));
        assert!(tabs.show(issue("a")));
        assert!(tabs.show(ZapperPage::File("NOTES.md".into())));
        assert_eq!(
            tabs.open_tabs(),
            [issue("a"), ZapperPage::File("NOTES.md".into())]
        );
        // Re-opening focuses the existing tab instead of adding another.
        assert!(tabs.show(issue("a")));
        assert_eq!(tabs.open_tabs().len(), 2);
        assert_eq!(tabs.active_issue(), Some("a"));
        // The section it was opened from keeps its label.
        assert_eq!(tabs.section(), ZapperTab::Issues);
        assert!(!tabs.show(issue("a")));
    }

    #[test]
    fn only_the_active_section_is_expanded() {
        let mut tabs = ZapperPanelTabs::default();
        assert_eq!(tabs.expanded_section(), Some(ZapperTab::Overview));
        tabs.select(ZapperTab::Issues);
        assert_eq!(tabs.expanded_section(), Some(ZapperTab::Issues));
        // An issue or file tab collapses every section to an icon.
        tabs.show(issue("a"));
        assert_eq!(tabs.expanded_section(), None);
        tabs.show(ZapperPage::File("NOTES.md".into()));
        assert_eq!(tabs.expanded_section(), None);
        tabs.show(ZapperPage::History);
        assert_eq!(tabs.expanded_section(), None);
        // The section it came from is still remembered for closing back.
        assert_eq!(tabs.section(), ZapperTab::Issues);
        tabs.close(&ZapperPage::History);
        tabs.close(&ZapperPage::File("NOTES.md".into()));
        tabs.close(&issue("a"));
        assert_eq!(tabs.expanded_section(), Some(ZapperTab::Issues));
    }

    #[test]
    fn closing_returns_to_the_tab_shown_before() {
        let mut tabs = ZapperPanelTabs::default();
        tabs.select(ZapperTab::Drive);
        tabs.show(ZapperPage::File("NOTES.md".into()));
        tabs.select(ZapperTab::Issues);
        tabs.show(issue("a"));
        tabs.show(issue("b"));
        assert!(tabs.close(&issue("b")));
        assert_eq!(tabs.active(), &issue("a"));
        assert!(tabs.close(&issue("a")));
        assert_eq!(tabs.active(), &ZapperPage::Section(ZapperTab::Issues));
        // Closing a background tab leaves the active page alone.
        assert!(tabs.close(&ZapperPage::File("NOTES.md".into())));
        assert_eq!(tabs.active(), &ZapperPage::Section(ZapperTab::Issues));
        assert!(tabs.open_tabs().is_empty());
        // Sections and unknown pages cannot be closed.
        assert!(!tabs.close(&ZapperPage::Section(ZapperTab::Issues)));
        assert!(!tabs.close(&issue("z")));
    }

    #[test]
    fn closing_skips_pages_already_closed() {
        let mut tabs = ZapperPanelTabs::default();
        tabs.show(issue("a"));
        tabs.show(ZapperPage::History);
        tabs.show(issue("b"));
        tabs.close(&ZapperPage::History);
        tabs.close(&issue("b"));
        assert_eq!(tabs.active(), &issue("a"));
        tabs.close(&issue("a"));
        assert_eq!(tabs.active(), &ZapperPage::Section(ZapperTab::Overview));
    }

    #[test]
    fn reset_returns_to_a_bare_overview() {
        let mut tabs = ZapperPanelTabs::default();
        tabs.select(ZapperTab::Threads);
        tabs.show(issue("a"));
        tabs.reset();
        assert_eq!(tabs, ZapperPanelTabs::default());
    }

    #[test]
    fn a_hidden_section_falls_back_to_overview() {
        let mut tabs = ZapperPanelTabs::default();
        tabs.select(ZapperTab::Drive);
        assert!(tabs.hide_section(ZapperTab::Drive));
        assert_eq!(tabs.active(), &ZapperPage::Section(ZapperTab::Overview));
        tabs.select(ZapperTab::Drive);
        tabs.show(issue("a"));
        assert!(tabs.hide_section(ZapperTab::Drive));
        assert_eq!(tabs.active(), &issue("a"));
        assert_eq!(tabs.section(), ZapperTab::Overview);
        // Closing the tab no longer returns to the hidden section.
        tabs.close(&issue("a"));
        assert_eq!(tabs.active(), &ZapperPage::Section(ZapperTab::Overview));
        assert!(!tabs.hide_section(ZapperTab::Drive));
    }

    #[test]
    fn history_is_not_a_section() {
        assert!(!ZAPPER_SECTIONS.contains(&ZapperTab::History));
        assert!(ZapperPage::History.closable());
        assert!(!ZapperPage::Section(ZapperTab::Drive).closable());
    }

    fn raised(
        number: u32,
        status: crate::zappers::ZapperIssueStatus,
        owner: Option<crate::zappers::ZapperIssueOwner>,
        activity: &[(&str, &str)],
    ) -> crate::zappers::ZapperIssue {
        crate::zappers::ZapperIssue {
            id: format!("issue-{number}"),
            number,
            details: crate::zappers::ZapperIssueDetails {
                title: format!("Issue {number}"),
                status,
                owner,
                ..Default::default()
            },
            created_at: 0,
            updated_at: 0,
            threads: Vec::new(),
            pull_requests: Vec::new(),
            activity: activity
                .iter()
                .enumerate()
                .map(|(n, (kind, text))| crate::zappers::ZapperIssueActivity {
                    actor: None,
                    id: format!("a{n}"),
                    created_at: n as i64,
                    kind: (*kind).into(),
                    text: (*text).into(),
                    reply_to: None,
                })
                .collect(),
        }
    }
    fn decision(id: &str, answer: Option<&str>) -> crate::zappers::ZapperDecision {
        crate::zappers::ZapperDecision {
            id: id.into(),
            worker_chat_id: None,
            question: format!("Question {id}?"),
            answer: answer.map(str::to_owned),
        }
    }

    fn sample_zapper(workers: &[&str], issue_threads: &[&[&str]]) -> crate::zappers::Zapper {
        use crate::zappers::*;
        Zapper {
            id: "z".into(),
            revision: 1,
            activity_revision: 0,
            config: ZapperConfig {
                name: "n".into(),
                purpose: String::new(),
                home_device_id: "d".into(),
                repositories: vec![],
                coordinator_model: ZapperModel {
                    harness: "mock".into(),
                    model: None,
                },
                worker_device_id: "d".into(),
                max_concurrent_workers: 1,
                max_followups: 1,
                avatar: None,
                avatar_image: None,
            },
            status: ZapperStatus::Active,
            coordinator_chat_id: "zapper-z-2".into(),
            previous_coordinator_chats: vec!["zapper-z-1".into()],
            workers: workers
                .iter()
                .map(|id| ZapperWorker {
                    chat_id: (*id).into(),
                    device_id: "d".into(),
                    space_id: "s".into(),
                    objective: format!("Objective {id}"),
                    followups: 0,
                    queue_position: None,
                })
                .collect(),
            decisions: vec![],
            issues: issue_threads
                .iter()
                .enumerate()
                .map(|(n, threads)| {
                    let mut issue = raised(n as u32 + 1, ZapperIssueStatus::InProgress, None, &[]);
                    issue.threads = threads.iter().map(|t| (*t).into()).collect();
                    issue
                })
                .collect(),
        }
    }

    #[test]
    fn threads_list_working_then_queued_then_idle_newest_first() {
        let mut zapper = sample_zapper(&["a", "b", "c", "d", "e", "f"], &[]);
        zapper.workers[1].queue_position = Some(2);
        zapper.workers[4].queue_position = Some(1);
        let rank = |w: &crate::zappers::ZapperWorker| match w.chat_id.as_str() {
            "a" | "d" => ThreadRank::Working,
            "c" => ThreadRank::NeedsYou,
            // A queued thread has no chat; whatever the host says, it's queued.
            "e" => ThreadRank::Working,
            _ => ThreadRank::Idle,
        };
        let order: Vec<&str> = ordered_zapper_threads(zapper.workers.iter(), rank)
            .into_iter()
            .map(|w| w.chat_id.as_str())
            .collect();
        assert_eq!(order, ["d", "a", "c", "e", "b", "f"]);
    }

    #[test]
    fn zapper_threads_stay_out_of_projects_once_released() {
        let zappers = [sample_zapper(&["zapper-worker-running", "adopted"], &[])];
        let keeps = |id: &str| zapper_keeps_chat(&zappers, id, None, None);
        // Running threads and every coordinator conversation.
        assert!(keeps("zapper-worker-running"));
        assert!(keeps("adopted"));
        assert!(keeps("zapper-z-2"));
        assert!(keeps("zapper-z-1"));
        // Released threads are gone from `workers` (including ones released
        // before this check), yet still belong to the Zapper.
        assert!(keeps("zapper-worker-delegate-zap11-ci-fix-1"));
        assert!(zapper_keeps_chat(&[], "zapper-worker-old", None, None));
        // Side chats of a thread, running or released, follow it.
        assert!(zapper_keeps_chat(
            &zappers,
            "side",
            Some("zapper-worker-running"),
            None
        ));
        assert!(zapper_keeps_chat(
            &zappers,
            "side",
            Some("zapper-worker-gone"),
            None
        ));
        assert!(zapper_keeps_chat(
            &zappers,
            "tab",
            None,
            Some("zapper-worker-gone")
        ));
        // Ordinary chats, and an adopted chat after its release, are projects'.
        assert!(!keeps("plain"));
        assert!(!zapper_keeps_chat(&[], "adopted", None, None));
        assert!(!zapper_keeps_chat(&zappers, "side", Some("plain"), None));
        assert!(zapper_thread_chat_id("zapper-worker-1"));
        assert!(!zapper_thread_chat_id("zapper-z-1"));
        // Ids minted before the rename keep their Zapper (legacy, remove
        // after one release).
        assert!(zapper_keeps_chat(&[], "zot-worker-old", None, None));
        assert!(zapper_thread_chat_id("zot-worker-1"));
        assert!(!zapper_thread_chat_id("zot-z-1"));
        assert!(zapper_chat_id("zot-z-1") && zapper_chat_id("zapper-z-1"));
    }

    #[test]
    fn finished_threads_are_released_issue_threads_newest_first() {
        let zapper = sample_zapper(
            &["zapper-worker-running"],
            &[
                &["zapper-worker-a", "zapper-worker-running"],
                &[
                    "zapper-worker-b",
                    "adopted",
                    "zapper-worker-a",
                    "zapper-worker-c",
                ],
            ],
        );
        assert_eq!(
            finished_zapper_threads(&zapper),
            ["zapper-worker-c", "zapper-worker-a", "zapper-worker-b"]
        );
        assert!(
            finished_zapper_threads(&sample_zapper(
                &["zapper-worker-a"],
                &[&["zapper-worker-a"]]
            ))
            .is_empty()
        );
    }

    #[test]
    fn needs_you_lists_only_user_asks() {
        use crate::zappers::{ZapperIssueOwner as O, ZapperIssueStatus as S};
        let issues = [
            raised(
                1,
                S::NeedsYou,
                Some(O::User),
                &[("comment", "fyi"), ("question", "Ship it?")],
            ),
            // Not the user's: the Zapper and its threads hold these.
            raised(2, S::InProgress, Some(O::Zapper), &[("question", "old")]),
            raised(
                3,
                S::InProgress,
                Some(O::Thread {
                    chat_id: "c".into(),
                }),
                &[],
            ),
            // Handed back or finished.
            raised(4, S::Todo, Some(O::Zapper), &[("question", "answered")]),
            raised(5, S::Done, Some(O::User), &[]),
        ];
        let decisions = [decision("d1", None), decision("d2", Some("yes"))];
        let items = needs_you_items(&issues, &decisions);
        assert_eq!(
            items,
            [
                NeedsYouItem::Issue {
                    id: "issue-1".into(),
                    code: "ZAP-1".into(),
                    title: "Issue 1".into(),
                    question: Some("Ship it?".into()),
                },
                NeedsYouItem::Decision {
                    id: "d1".into(),
                    question: "Question d1?".into(),
                },
            ]
        );
        assert_eq!(items[0].title(), "ZAP-1 · Ship it?");
    }

    #[test]
    fn needs_you_issue_without_a_question_shows_its_title() {
        use crate::zappers::{ZapperIssueOwner as O, ZapperIssueStatus as S};
        let items = needs_you_items(&[raised(7, S::NeedsYou, Some(O::User), &[])], &[]);
        assert_eq!(items[0].title(), "ZAP-7 · Issue 7");
    }

    #[test]
    fn needs_you_items_leave_once_answered_or_handed_back() {
        use crate::zappers::{ZapperIssueOwner as O, ZapperIssueStatus as S};
        let mut asked = raised(1, S::NeedsYou, Some(O::User), &[("question", "Ok?")]);
        let mut open = decision("d", None);
        assert_eq!(needs_you_items(&[asked.clone()], &[open.clone()]).len(), 2);
        open.answer = Some("yes".into());
        asked.details.owner = Some(O::Zapper);
        asked.details.status = S::InProgress;
        assert!(needs_you_items(&[asked], &[open]).is_empty());
    }

    #[test]
    fn open_routes_to_the_issue_tab_or_the_decision_form() {
        let item = NeedsYouItem::Issue {
            id: "i".into(),
            code: "ZAP-1".into(),
            title: "t".into(),
            question: None,
        };
        assert_eq!(item.target(), NeedsYouTarget::IssueTab("i".into()));
        let ask = NeedsYouItem::Decision {
            id: "d".into(),
            question: "q".into(),
        };
        assert_eq!(ask.target(), NeedsYouTarget::DecisionForm("d".into()));
    }
}
