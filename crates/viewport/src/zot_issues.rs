//! The same issue surface is embedded by the browser and native Zot inspectors.
use crate::tooltip::TooltipExt as _;
use crate::zapper_avatar::{ZapperFace, known_face, person_avatar, unassigned_avatar, zapper_face};
use crate::{assets::icon, input::TextInput, palette::rgb};
use gpui::{prelude::*, *};
use std::collections::{HashMap, HashSet};
use zuse_protocol::zots::*;

const TEXT: u32 = 0xe5e5e5;
const MUTED: u32 = 0x777777;
const BORDER: u32 = 0x343434;
const FAINT: u32 = 0x5c5c5c;
const HOVER: u32 = 0x1f211f;
const MENU: u32 = 0x1c1d19;
#[derive(Clone)]
pub enum IssueEvent {
    Command(ZotAction, u32),
    OpenThread(String),
    OpenDocument(ZotFile),
    OpenPullRequest(String),
    /// The overview asks the host to start a thread for this Zapper.
    NewThread,
    /// The overview's "View all" asks the host to switch to the Issues page.
    ShowIssues,
}
/// How a Zapper thread is doing, as the host sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Working,
    NeedsYou,
    Idle,
    /// Archived threads leave the overview.
    Archived,
}
pub struct IssuePanel {
    zot: Zot,
    thread_titles: HashMap<String, String>,
    overview: bool,
    selected: Option<String>,
    needs_you_only: bool,
    collapsed: HashSet<String>,
    thread_states: HashMap<String, ThreadState>,
    /// The signed-in person's name, for their avatar on comments.
    user_name: Option<String>,
    menu: Option<&'static str>,
    editor: Option<Editor>,
    search: Option<Entity<TextInput>>,
    searching: bool,
    pending: bool,
    error: Option<String>,
    repository: Option<ZotRepository>,
    files: Vec<ZotFile>,
    activity_filter: u8,
    /// Activity cards opened past their preview length.
    expanded: HashSet<String>,
}
struct Editor {
    kind: EditorKind,
    fields: Vec<Entity<TextInput>>,
    create_id: String,
    _submit: Option<Subscription>,
}
/// One issue property edited in place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Description,
    DueDate,
    Labels,
    DoneCriteria,
    OutOfScope,
}
#[derive(Clone)]
enum EditorKind {
    Create,
    Field(String, Field),
    Comment(String),
    /// Issue, then the activity entry being answered.
    Reply(String, String),
    PullRequest(String),
    Delegate(String),
}
impl EditorKind {
    /// Inline editors render inside the issue page instead of replacing it.
    fn inline(&self) -> bool {
        matches!(self, Self::Field(..) | Self::Comment(_) | Self::Reply(..))
    }
}
impl EventEmitter<IssueEvent> for IssuePanel {}
impl IssuePanel {
    pub fn new(zot: Zot, overview: bool) -> Self {
        Self {
            zot,
            thread_titles: HashMap::new(),
            overview,
            selected: None,
            needs_you_only: false,
            thread_states: HashMap::new(),
            user_name: None,
            collapsed: ["Done".to_owned(), "Canceled".to_owned()]
                .into_iter()
                .collect(),
            menu: None,
            editor: None,
            search: None,
            searching: false,
            pending: false,
            error: None,
            repository: None,
            files: vec![],
            activity_filter: 0,
            expanded: HashSet::new(),
        }
    }
    pub fn sync(&mut self, zot: Zot, overview: bool, pending: bool, cx: &mut Context<Self>) {
        if self.zot != zot || self.overview != overview || self.pending != pending {
            // Tab switches reset the page through `show_list`; only a
            // different Zapper drops the open issue here, so "Answer" in
            // the overview can open an issue on the Issues page.
            if self.zot.id != zot.id {
                self.selected = None;
                self.editor = None;
                self.menu = None;
            } else if self.overview != overview {
                self.menu = None;
            }
            self.zot = zot;
            self.overview = overview;
            self.pending = pending;
            cx.notify();
        }
    }
    pub fn zot_id(&self) -> &str {
        &self.zot.id
    }
    pub fn selected_code(&self) -> Option<String> {
        self.selected
            .as_ref()
            .and_then(|id| self.zot.issues.iter().find(|i| i.id == *id))
            .map(ZotIssue::code)
    }
    /// The issue on show, by id. Hosts that open issues as tabs follow it.
    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn show_issue(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.editor = None;
        self.menu = None;
        cx.notify();
    }
    pub fn show_list(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.editor = None;
        self.menu = None;
        cx.notify();
    }
    pub fn files(&mut self, files: Vec<ZotFile>, cx: &mut Context<Self>) {
        if self.files != files {
            self.files = files;
            cx.notify();
        }
    }
    pub fn thread_titles(&mut self, titles: HashMap<String, String>, cx: &mut Context<Self>) {
        if self.thread_titles != titles {
            self.thread_titles = titles;
            cx.notify();
        }
    }
    pub fn thread_states(&mut self, states: HashMap<String, ThreadState>, cx: &mut Context<Self>) {
        if self.thread_states != states {
            self.thread_states = states;
            cx.notify();
        }
    }
    pub fn user_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.user_name != name {
            self.user_name = name;
            cx.notify();
        }
    }
    fn thread_state(&self, id: &str) -> ThreadState {
        self.thread_states
            .get(id)
            .copied()
            .unwrap_or(ThreadState::Idle)
    }
    /// The owner's picture: you, this Zapper's face (also for its threads),
    /// or an empty ring.
    fn owner_avatar(&self, owner: Option<&ZotIssueOwner>, size: f32) -> Div {
        match owner {
            Some(ZotIssueOwner::User) => {
                person_avatar(self.user_name.as_deref().unwrap_or("You"), size)
            }
            Some(ZotIssueOwner::Zapper | ZotIssueOwner::Thread { .. }) => {
                zapper_face(&self.avatar(), size)
            }
            None => unassigned_avatar(size),
        }
    }
    fn owner_name(&self, owner: Option<&ZotIssueOwner>) -> String {
        match owner {
            Some(ZotIssueOwner::User) => "You".into(),
            Some(ZotIssueOwner::Zapper) => self.zot.config.name.clone(),
            Some(ZotIssueOwner::Thread { chat_id }) => self.thread_title(chat_id),
            None => "No owner".into(),
        }
    }
    fn avatar(&self) -> ZapperFace {
        ZapperFace::of(&self.zot)
    }
    fn thread_title(&self, id: &str) -> String {
        self.thread_titles
            .get(id)
            .cloned()
            .or_else(|| {
                self.zot.workers.iter().find(|w| w.chat_id == id).map(|w| {
                    w.objective
                        .lines()
                        .next()
                        .unwrap_or("Worker thread")
                        .chars()
                        .take(80)
                        .collect()
                })
            })
            .unwrap_or_else(|| "Earlier thread".into())
    }
    pub fn failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message);
        self.pending = false;
        cx.notify();
    }
    pub fn selected_status(&self) -> Option<ZotIssueStatus> {
        self.selected
            .as_ref()
            .and_then(|id| self.zot.issues.iter().find(|i| i.id == *id))
            .map(|i| i.details.status)
    }
    fn emit(&mut self, action: ZotAction, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        self.error = None;
        cx.emit(IssueEvent::Command(action, self.zot.revision));
        cx.notify();
    }
    fn open(&mut self, kind: EditorKind, window: &mut Window, cx: &mut Context<Self>) {
        let issue_id = match &kind {
            EditorKind::Create => None,
            EditorKind::Field(id, _)
            | EditorKind::Comment(id)
            | EditorKind::Reply(id, _)
            | EditorKind::PullRequest(id)
            | EditorKind::Delegate(id) => Some(id),
        };
        let issue = issue_id.and_then(|id| self.zot.issues.iter().find(|i| i.id == *id));
        let fields: Vec<(&str, String, bool)> = match &kind {
            EditorKind::Create => vec![
                ("Issue title", String::new(), false),
                ("Description", String::new(), true),
                ("What must be true for this to be done", String::new(), true),
            ],
            EditorKind::Field(_, field) => {
                let Some(i) = issue else {
                    return;
                };
                vec![match field {
                    Field::Title => ("Issue title", i.details.title.clone(), false),
                    Field::Description => {
                        ("Add a description…", i.details.description.clone(), true)
                    }
                    Field::DueDate => (
                        "YYYY-MM-DD",
                        i.details.due_date.clone().unwrap_or_default(),
                        false,
                    ),
                    Field::Labels => ("bug, ux", i.details.labels.join(", "), false),
                    Field::DoneCriteria => (
                        "What must be true for this to be done…",
                        i.details.done_criteria.clone(),
                        true,
                    ),
                    Field::OutOfScope => (
                        "What this issue leaves alone…",
                        i.details.out_of_scope.clone(),
                        true,
                    ),
                }]
            }
            EditorKind::Comment(_) => vec![("Leave a comment…", String::new(), true)],
            EditorKind::Reply(..) => vec![("Reply…", String::new(), true)],
            EditorKind::PullRequest(_) => vec![
                (
                    "https://github.com/owner/repository/pull/123",
                    String::new(),
                    false,
                ),
                ("Pull request title", String::new(), false),
            ],
            EditorKind::Delegate(_) => vec![
                (
                    "Assignment",
                    issue
                        .map(|i| format!("{}\n\n{}", i.details.title, i.details.description))
                        .unwrap_or_default(),
                    true,
                ),
                ("Base branch", "HEAD".into(), false),
            ],
        };
        let inputs: Vec<_> = fields
            .into_iter()
            .map(|(label, value, multi)| {
                let title = matches!(kind, EditorKind::Field(_, Field::Title));
                let input = cx.new(|cx| {
                    if multi {
                        TextInput::multiline(label, window, cx)
                    } else if title {
                        // The title edits at the size it reads at.
                        TextInput::new(label, window, cx).with_single_line_metrics(20., 25.)
                    } else {
                        TextInput::new(label, window, cx)
                    }
                });
                input.update(cx, |i, cx| i.set_value(value, cx));
                input
            })
            .collect();
        if let Some(first) = inputs.first() {
            first.update(cx, |i, cx| i.focus(cx));
        }
        // Inline fields save on Enter (Shift+Enter keeps a newline).
        let submit = inputs.first().filter(|_| kind.inline()).map(|input| {
            cx.subscribe(input, |this, _, _: &crate::input::Submitted, cx| {
                this.submit(cx)
            })
        });
        self.editor = Some(Editor {
            kind,
            fields: inputs,
            create_id: uuid::Uuid::new_v4().to_string(),
            _submit: submit,
        });
        self.menu = None;
        self.error = None;
        cx.notify();
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let values: Vec<_> = editor.fields.iter().map(|i| i.read(cx).value(cx)).collect();
        let kind = editor.kind.clone();
        let action = match kind {
            EditorKind::Create => {
                if values[0].trim().is_empty() {
                    self.error = Some("Give the issue a title".into());
                    cx.notify();
                    return;
                }
                let id = editor.create_id.clone();
                self.selected = Some(id.clone());
                ZotAction::CreateIssue {
                    issue_id: id,
                    details: ZotIssueDetails {
                        title: values[0].trim().into(),
                        description: values[1].clone(),
                        done_criteria: values[2].trim().into(),
                        ..Default::default()
                    },
                }
            }
            EditorKind::Field(id, field) => {
                let Some(issue) = self.zot.issues.iter().find(|i| i.id == id) else {
                    return;
                };
                let value = values[0].trim();
                let mut details = issue.details.clone();
                match field {
                    Field::Title => {
                        if value.is_empty() {
                            self.error = Some("Give the issue a title".into());
                            cx.notify();
                            return;
                        }
                        details.title = value.into();
                    }
                    Field::Description => details.description = values[0].trim_end().into(),
                    Field::DoneCriteria => details.done_criteria = values[0].trim_end().into(),
                    Field::OutOfScope => details.out_of_scope = values[0].trim_end().into(),
                    Field::DueDate => {
                        if !value.is_empty()
                            && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err()
                        {
                            self.error = Some("Use a date like 2026-12-31".into());
                            cx.notify();
                            return;
                        }
                        details.due_date = (!value.is_empty()).then(|| value.into());
                    }
                    Field::Labels => {
                        details.labels = value
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect();
                    }
                }
                if details == issue.details {
                    // Nothing changed: just close the field.
                    self.editor = None;
                    cx.notify();
                    return;
                }
                ZotAction::EditIssue {
                    issue_id: id,
                    details,
                }
            }
            EditorKind::Comment(_) | EditorKind::Reply(..) if values[0].trim().is_empty() => {
                return;
            }
            EditorKind::Comment(id) => ZotAction::CommentIssue {
                issue_id: id,
                text: values[0].clone(),
                reply_to: None,
            },
            EditorKind::Reply(id, parent) => ZotAction::CommentIssue {
                issue_id: id,
                text: values[0].clone(),
                reply_to: Some(parent),
            },
            EditorKind::PullRequest(id) => ZotAction::LinkIssuePullRequest {
                issue_id: id,
                url: values[0].clone(),
                title: values[1].clone(),
            },
            EditorKind::Delegate(id) => {
                if !self
                    .zot
                    .issues
                    .iter()
                    .any(|i| i.id == id && i.details.assigned && !i.details.status.terminal())
                {
                    self.error =
                        Some("Assign and reopen this issue before starting a thread".into());
                    cx.notify();
                    return;
                }
                if self.zot.status != ZotStatus::Active {
                    self.error =
                        Some("Resume this Zapper from Threads before starting work".into());
                    cx.notify();
                    return;
                }
                let Some(repo) = self
                    .repository
                    .as_ref()
                    .filter(|r| self.zot.config.repositories.contains(r))
                    .or(self.zot.config.repositories.first())
                else {
                    self.error = Some("Add a repository to this Zapper first".into());
                    cx.notify();
                    return;
                };
                ZotAction::Delegate {
                    issue_id: Some(id),
                    worker: ZotWorker {
                        chat_id: String::new(),
                        device_id: repo.device_id.clone(),
                        space_id: repo.space_id.clone(),
                        objective: values[0].clone(),
                        followups: 0,
                        queue_position: None,
                    },
                    model: self.zot.config.coordinator_model.clone(),
                    // Same fallback as the other delegation paths.
                    base: match values[1].trim() {
                        "" => "HEAD".into(),
                        base => base.into(),
                    },
                }
            }
        };
        // Keep the draft until the authoritative watch confirms its write.
        self.emit(action, cx);
    }
    fn issue_row(&self, issue: &ZotIssue, cx: &mut Context<Self>) -> AnyElement {
        let id = issue.id.clone();
        let owner = issue.details.current_owner();
        let assignee = self.owner_name(owner.as_ref());
        // Its thread waits for a slot: show Queued, not a running status.
        let queued = self.zot.issue_queued_worker(issue).is_some();
        row()
            .id(SharedString::from(format!("issue-row-{id}")))
            .role(Role::Button)
            .aria_label(SharedString::from(format!(
                "{} {}",
                issue.code(),
                issue.details.title
            )))
            .w_full()
            .h(px(34.))
            .gap(px(10.))
            .px(px(10.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(HOVER)))
            .child(priority_icon(issue.details.priority))
            .child(status_icon(if queued {
                ZotIssueStatus::Todo
            } else {
                issue.details.status
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(issue.details.title.clone()),
            )
            .when(queued, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(rgb(MUTED))
                        .child("Queued"),
                )
            })
            .when_some(issue.pull_requests.first(), |d, pr| d.child(pr_badge(pr)))
            .child(
                div()
                    .id(SharedString::from(format!("issue-row-assignee-{id}")))
                    .tip(assignee)
                    .child(self.owner_avatar(owner.as_ref(), 16.)),
            )
            .child(
                div()
                    .w(px(30.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .text_size(px(11.5))
                    .text_color(rgb(FAINT))
                    .child(short_time(issue.updated_at)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id.clone());
                this.editor = None;
                this.menu = None;
                cx.notify();
            }))
            .into_any_element()
    }
    fn matches_query(&self, issue: &ZotIssue, query: &str) -> bool {
        query.is_empty()
            || format!(
                "{} {} {}",
                issue.code(),
                issue.details.title,
                issue.details.description
            )
            .to_lowercase()
            .contains(query)
    }
    /// The Issues page: filter tabs, then issues grouped by status.
    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.overview {
            return self.overview_page(cx);
        }
        let mut body = column().gap(px(2.));
        let waiting = self
            .zot
            .issues
            .iter()
            .filter(|i| i.details.status == ZotIssueStatus::NeedsYou)
            .count();
        let mut tabs = row().gap(px(2.));
        for (index, label, count) in [(0, "All", self.zot.issues.len()), (1, "Needs you", waiting)]
        {
            let selected = (index == 1) == self.needs_you_only;
            tabs = tabs.child(
                button(format!("issues-filter-{index}"), "")
                    .aria_label(label)
                    .h(px(28.))
                    .py(px(0.))
                    .px(px(10.))
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .when(selected, |d| d.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                    .when(!selected, |d| d.text_color(rgb(MUTED)))
                    .when(index == 1, |d| {
                        d.child(self.owner_avatar(Some(&ZotIssueOwner::User), 14.))
                    })
                    .child(label)
                    .child(
                        div()
                            .text_color(rgb(FAINT))
                            .text_size(px(11.5))
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.needs_you_only = index == 1;
                        cx.notify();
                    })),
            );
        }
        let icon_button = |id: &'static str, label: &'static str, glyph: &'static str| {
            plain_button(id, "")
                .aria_label(label)
                .tip(label)
                .size(px(28.))
                .p(px(0.))
                .justify_center()
                .text_color(rgb(MUTED))
                .hover(|d| d.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                .child(icon(glyph).size(px(14.)))
        };
        body =
            body.child(
                row()
                    .pb(px(8.))
                    .child(tabs)
                    .child(div().flex_1())
                    .child(
                        icon_button("issues-search", "Search issues", "magnifer")
                            .when(self.searching, |d| d.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.searching = !this.searching;
                                if this.searching {
                                    if let Some(input) = &this.search {
                                        input.update(cx, |i, cx| i.focus(cx));
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .child(icon_button("issues-create", "New issue", "plus").on_click(
                        cx.listener(|this, _, w, cx| this.open(EditorKind::Create, w, cx)),
                    )),
            );
        if self.searching {
            if let Some(search) = &self.search {
                body = body.child(div().pb(px(8.)).child(input_box(search.clone(), 34.)));
            }
        }
        if self.zot.issues.is_empty() {
            return body
                .child(self.empty_state(
                    "No issues yet",
                    "Write down what needs doing and assign it to this Zapper. It starts a thread for each issue and reports back here.",
                    false,
                    cx,
                ))
                .into_any_element();
        }
        let query = self
            .search
            .as_ref()
            .filter(|_| self.searching)
            .map(|i| i.read(cx).value(cx).to_lowercase())
            .unwrap_or_default();
        for status in ZotIssueStatus::BOARD {
            if self.needs_you_only && status != ZotIssueStatus::NeedsYou {
                continue;
            }
            let issues: Vec<_> = self
                .zot
                .issues
                .iter()
                .filter(|i| i.details.status == status && self.matches_query(i, &query))
                .collect();
            // Done and Canceled keep a header so finished work stays findable.
            if issues.is_empty() && !status.terminal() {
                continue;
            }
            let key = status.label().to_owned();
            let closed = self.collapsed.contains(&key);
            body = body.child(section(status, issues.len(), &key, closed, cx));
            if !closed {
                for i in issues {
                    body = body.child(self.issue_row(i, cx));
                }
            }
        }
        if self.needs_you_only && waiting == 0 {
            body = body.child(
                div()
                    .px(px(10.))
                    .py(px(20.))
                    .text_color(rgb(MUTED))
                    .child("Nothing needs you right now. When the Zapper needs a decision, it hands the issue to you here."),
            );
        }
        body.into_any_element()
    }
    /// A calm placeholder with the next step as a button.
    fn empty_state(
        &self,
        title: &'static str,
        body: &'static str,
        with_thread: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        column()
            .items_center()
            .gap(px(6.))
            .px(px(24.))
            .py(px(36.))
            .rounded(px(10.))
            .border_1()
            .border_dashed()
            .border_color(rgb(0x2a2a2a))
            .child(zapper_face(&self.avatar(), 36.).mb(px(6.)))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT))
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(320.))
                    .text_center()
                    .text_size(px(12.5))
                    .line_height(px(18.))
                    .text_color(rgb(MUTED))
                    .child(body),
            )
            .child(
                row()
                    .gap(px(8.))
                    .pt(px(10.))
                    .child(
                        plain_button("empty-new-issue", "")
                            .aria_label("New issue")
                            .h(px(28.))
                            .px(px(12.))
                            .py(px(0.))
                            .gap(px(6.))
                            .bg(rgb(0xe8e9e7))
                            .text_color(rgb(0x111111))
                            .hover(|d| d.bg(rgb(0xebebeb)))
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .child(icon("plus").size(px(12.)).text_color(rgb(0x111111)))
                            .child("New issue")
                            .on_click(
                                cx.listener(|this, _, w, cx| this.open(EditorKind::Create, w, cx)),
                            ),
                    )
                    .when(with_thread, |d| {
                        d.child(
                            button("empty-new-thread", "")
                                .aria_label("New thread")
                                .h(px(28.))
                                .px(px(12.))
                                .py(px(0.))
                                .gap(px(6.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .text_size(px(12.5))
                                .child(icon("chat-round-line").size(px(12.)))
                                .child("New thread")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(IssueEvent::NewThread)),
                                ),
                        )
                    }),
            )
    }
    /// A titled block of the overview, with an optional trailing action.
    fn block(&self, title: &str, count: usize, action: Option<AnyElement>) -> Div {
        column().gap(px(2.)).child(
            row()
                .h(px(26.))
                .px(px(10.))
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(title.to_owned())
                .child(div().text_color(rgb(FAINT)).child(count.to_string()))
                .child(div().flex_1())
                .children(action),
        )
    }
    fn link(&self, id: &'static str, label: &'static str) -> Stateful<Div> {
        plain_button(id, label)
            .h(px(22.))
            .py(px(0.))
            .px(px(6.))
            .text_size(px(12.))
            .text_color(rgb(MUTED))
            .hover(|d| d.bg(rgb(HOVER)).text_color(rgb(TEXT)))
    }
    /// The overview: open work first, then live threads, pull requests and
    /// documents. Empty blocks are left out instead of showing a zero.
    fn overview_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let open: Vec<&ZotIssue> = ZotIssueStatus::BOARD
            .iter()
            .filter(|s| !s.terminal())
            .flat_map(|s| {
                self.zot
                    .issues
                    .iter()
                    .filter(move |i| i.details.status == *s)
            })
            .collect();
        let finished = self.zot.issues.len() - open.len();
        let threads: Vec<&ZotWorker> = self
            .zot
            .workers
            .iter()
            .filter(|w| self.thread_state(&w.chat_id) != ThreadState::Archived)
            .collect();
        let prs: Vec<_> = self
            .zot
            .issues
            .iter()
            .filter(|i| !i.details.status.terminal())
            .flat_map(|i| i.pull_requests.iter())
            .collect();
        let files: Vec<_> = self
            .files
            .iter()
            .filter(|f| {
                (f.path != "INSTRUCTIONS.md" || f.version > 1) && !f.content.trim().is_empty()
            })
            .collect();
        let mut body = column().gap(px(18.));
        if open.is_empty() && threads.is_empty() {
            body = body.child(self.empty_state(
                if finished > 0 { "All caught up" } else { "Nothing on the board yet" },
                if finished > 0 {
                    "Every issue is done. Add the next one, or start a thread for something quick."
                } else {
                    "Add an issue for this Zapper to pick up, or start a thread for something quick."
                },
                true,
                cx,
            ));
        }
        if !open.is_empty() {
            const SHOWN: usize = 8;
            let more = open.len() > SHOWN;
            let mut issues = self.block(
                "Open issues",
                open.len(),
                Some(
                    row()
                        .gap(px(2.))
                        .when(more || finished > 0, |d| {
                            d.child(self.link("overview-all-issues", "View all").on_click(
                                cx.listener(|_, _, _, cx| cx.emit(IssueEvent::ShowIssues)),
                            ))
                        })
                        .child(
                            plain_button("overview-new-issue", "")
                                .aria_label("New issue")
                                .tip("New issue")
                                .size(px(22.))
                                .p(px(0.))
                                .justify_center()
                                .text_color(rgb(MUTED))
                                .hover(|d| d.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                .child(icon("plus").size(px(12.)))
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.open(EditorKind::Create, w, cx)
                                })),
                        )
                        .into_any_element(),
                ),
            );
            for issue in open.into_iter().take(SHOWN) {
                issues = issues.child(self.issue_row(issue, cx));
            }
            body = body.child(issues);
        }
        if !threads.is_empty() {
            let mut list = self.block("Threads", threads.len(), None);
            for worker in threads {
                let id = worker.chat_id.clone();
                let state = self.thread_state(&id);
                let codes = self
                    .zot
                    .issues
                    .iter()
                    .filter(|i| i.threads.contains(&id))
                    .map(ZotIssue::code)
                    .collect::<Vec<_>>()
                    .join(", ");
                let (dot, label) = match state {
                    ThreadState::Working => (0x00b80a, "Working"),
                    ThreadState::NeedsYou => (0xd9bf60, "Needs you"),
                    _ => (0x4a4a4a, "Idle"),
                };
                list = list.child(
                    row()
                        .id(SharedString::from(format!("overview-thread-{id}")))
                        .h(px(34.))
                        .gap(px(10.))
                        .px(px(10.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgb(HOVER)))
                        .child(
                            div()
                                .size(px(12.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().size(px(7.)).rounded_full().bg(rgb(dot))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(self.thread_title(&id)),
                        )
                        .when(!codes.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(rgb(FAINT))
                                    .child(codes),
                            )
                        })
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.5))
                                .text_color(rgb(if state == ThreadState::Idle {
                                    FAINT
                                } else {
                                    MUTED
                                }))
                                .child(label),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(IssueEvent::OpenThread(id.clone()))
                        })),
                );
            }
            body = body.child(list);
        }
        if !prs.is_empty() {
            let mut list = self.block("Pull requests", prs.len(), None);
            for pr in prs {
                let url = pr.url.clone();
                list =
                    list.child(
                        row()
                            .id(SharedString::from(format!("overview-pr-{url}")))
                            .h(px(34.))
                            .gap(px(10.))
                            .px(px(10.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgb(HOVER)))
                            .child(icon("git-branch").size(px(13.)).text_color(rgb(0xd9bf60)))
                            .child(div().flex_1().min_w_0().truncate().child(
                                if pr.title.is_empty() {
                                    "Pull request".to_owned()
                                } else {
                                    pr.title.clone()
                                },
                            ))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(rgb(FAINT))
                                    .child(pr_label(pr)),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(IssueEvent::OpenPullRequest(url.clone()))
                            })),
                    );
            }
            body = body.child(list);
        }
        if !files.is_empty() {
            let mut list = self.block("Drive", files.len(), None);
            for file in files {
                let f = file.clone();
                let preview = file
                    .content
                    .lines()
                    .map(|l| l.trim_start_matches('#').trim())
                    .find(|l| !l.is_empty())
                    .unwrap_or_default()
                    .to_owned();
                list = list.child(
                    row()
                        .id(SharedString::from(format!("issue-document-{}", file.path)))
                        .h(px(34.))
                        .gap(px(10.))
                        .px(px(10.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgb(HOVER)))
                        .child(icon("document").size(px(13.)).text_color(rgb(MUTED)))
                        .child(div().flex_none().child(file.path.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(rgb(FAINT))
                                .child(preview),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(IssueEvent::OpenDocument(f.clone()))
                        })),
                );
            }
            body = body.child(list);
        }
        body.into_any_element()
    }
    fn detail(&self, issue: &ZotIssue, cx: &mut Context<Self>) -> AnyElement {
        let mut body = column().gap(px(8.)).pt(px(10.));
        let edit = issue.id.clone();
        // Click the title to edit it in place.
        let title = match self.editing(&issue.id, Field::Title) {
            Some(input) => inline_field(input, true, false, cx).into_any_element(),
            None => div()
                .id("issue-title")
                .role(Role::Button)
                .aria_label("Edit title")
                .mx(px(-6.))
                .px(px(6.))
                .rounded(px(6.))
                .cursor_text()
                .hover(|d| d.bg(rgb(0x181818)))
                .text_size(px(20.))
                .line_height(px(25.))
                .font_weight(FontWeight::MEDIUM)
                .child(issue.details.title.clone())
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.open(EditorKind::Field(edit.clone(), Field::Title), w, cx)
                }))
                .into_any_element(),
        };
        body = body.child(
            column().gap(px(8.)).child(title).child(
                div()
                    .text_color(rgb(MUTED))
                    .text_size(px(12.))
                    .child(format!(
                        "{}  |  {}",
                        issue.code(),
                        relative_time(issue.created_at)
                    )),
            ),
        );
        let mut properties = column().gap(px(0.)).mt(px(18.));
        for (glyph, label, value, menu) in [
            (
                "clock-circle",
                "Status",
                self.zot.issue_status_label(issue).to_owned(),
                "status",
            ),
            (
                "star",
                "Priority",
                issue.details.priority.label().to_owned(),
                "priority",
            ),
            (
                "calendar",
                "Due date",
                issue
                    .details
                    .due_date
                    .clone()
                    .unwrap_or_else(|| "No due date".into()),
                "due",
            ),
            (
                "bot",
                "Owner",
                self.owner_name(issue.details.current_owner().as_ref()),
                "assignee",
            ),
            (
                "tag",
                "Labels",
                if issue.details.labels.is_empty() {
                    "No labels".into()
                } else {
                    issue.details.labels.join(", ")
                },
                "labels",
            ),
        ] {
            let id = issue.id.clone();
            let field = match menu {
                "due" => Some(Field::DueDate),
                "labels" => Some(Field::Labels),
                _ => None,
            };
            let inline = field.and_then(|f| self.editing(&issue.id, f));
            let mut content = row().min_w_0().gap(px(6.));
            if menu == "status" {
                content = content.child(status_icon(issue.details.status));
            }
            if menu == "priority" {
                content = content.child(priority_icon(issue.details.priority));
            }
            if menu == "assignee" {
                content =
                    content.child(self.owner_avatar(issue.details.current_owner().as_ref(), 16.));
            }
            content = content
                .child(
                    div()
                        .truncate()
                        .when(value.starts_with("No "), |d| d.text_color(rgb(0x898989)))
                        .child(value.clone()),
                )
                .child(icon("alt-arrow-down").size(px(10.)).text_color(rgb(MUTED)));
            let mut property = row()
                .relative()
                .min_h(px(28.))
                .gap(px(8.))
                .text_size(px(12.))
                .line_height(px(18.))
                .child(if menu == "status" {
                    status_icon(ZotIssueStatus::Todo).into_any_element()
                } else {
                    icon(glyph)
                        .size(px(12.))
                        .text_color(rgb(MUTED))
                        .into_any_element()
                })
                .child(
                    div()
                        .w(px(84.))
                        .flex_none()
                        .text_color(rgb(MUTED))
                        .child(label),
                )
                .child(match inline {
                    Some(input) => div()
                        .flex_1()
                        .min_w_0()
                        .child(inline_field(input, false, false, cx))
                        .into_any_element(),
                    None => button(format!("issue-property-{label}"), "")
                        .aria_label(SharedString::from(format!("{label}: {value}")))
                        .flex_1()
                        .min_w_0()
                        .py(px(4.))
                        .child(content)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            if let Some(field) = field {
                                this.open(EditorKind::Field(id.clone(), field), w, cx);
                            } else {
                                this.menu = if this.menu == Some(menu) {
                                    None
                                } else {
                                    Some(menu)
                                };
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                });
            if self.menu == Some(menu) && field.is_none() {
                let mut options = menu_card(format!("issue-menu-{menu}"), 220.).on_mouse_down_out(
                    cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }),
                );
                // Owners: nobody, you, the Zapper, then threads on this issue.
                let owners: Vec<Option<ZotIssueOwner>> =
                    [None, Some(ZotIssueOwner::User), Some(ZotIssueOwner::Zapper)]
                        .into_iter()
                        .chain(
                            issue
                                .threads
                                .iter()
                                .filter(|id| self.zot.workers.iter().any(|w| w.chat_id == **id))
                                .map(|id| {
                                    Some(ZotIssueOwner::Thread {
                                        chat_id: id.clone(),
                                    })
                                }),
                        )
                        .collect();
                let count = match menu {
                    "status" => ZotIssueStatus::ALL.len(),
                    "priority" => ZotIssuePriority::ALL.len(),
                    _ => owners.len(),
                };
                for index in 0..count {
                    let mut details = issue.details.clone();
                    let (lead, label): (AnyElement, String) = match menu {
                        "status" => {
                            let status = ZotIssueStatus::ALL[index];
                            details.status = status;
                            (
                                status_icon(status).into_any_element(),
                                status.label().into(),
                            )
                        }
                        "priority" => {
                            let priority = ZotIssuePriority::ALL[index];
                            details.priority = priority;
                            (
                                priority_icon(priority).into_any_element(),
                                priority.label().into(),
                            )
                        }
                        _ => {
                            let owner = owners[index].clone();
                            details.assigned = matches!(
                                owner,
                                Some(ZotIssueOwner::Zapper | ZotIssueOwner::Thread { .. })
                            );
                            details.owner = owner.clone();
                            (
                                self.owner_avatar(owner.as_ref(), 18.).into_any_element(),
                                self.owner_name(owner.as_ref()),
                            )
                        }
                    };
                    let current = if menu == "assignee" {
                        details.current_owner() == issue.details.current_owner()
                    } else {
                        details == issue.details
                    };
                    let id = issue.id.clone();
                    options = options.child(
                        menu_option(format!("issue-option-{menu}-{index}"), lead, label, current)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                if current {
                                    cx.notify();
                                    return;
                                }
                                this.emit(
                                    ZotAction::EditIssue {
                                        issue_id: id.clone(),
                                        details: details.clone(),
                                    },
                                    cx,
                                );
                            })),
                    );
                }
                property = property.child(deferred(options).priority(3));
            }
            properties = properties.child(property);
        }
        body = body.child(properties);
        let mut links = column().gap(px(0.));
        for (label, glyph, kind) in [
            ("Pull request", "git-branch", 0),
            ("Thread", "chat-round-line", 1),
            ("Relations", "link", 2),
        ] {
            let mut values = column().flex_1().min_w_0().gap(px(0.));
            if kind == 0 {
                for pr in &issue.pull_requests {
                    let url = pr.url.clone();
                    let remove = ZotAction::UnlinkIssuePullRequest {
                        issue_id: issue.id.clone(),
                        url: pr.url.clone(),
                    };
                    values = values.child(
                        row()
                            .gap(px(2.))
                            .child(
                                button(format!("issue-pr-{url}"), "")
                                    .aria_label(SharedString::from(format!(
                                        "Open pull request {}",
                                        pr.title
                                    )))
                                    .flex_1()
                                    .min_w_0()
                                    .py(px(4.))
                                    .child(
                                        row()
                                            .gap(px(6.))
                                            .min_w_0()
                                            .child(
                                                icon("git-branch")
                                                    .size(px(13.))
                                                    .text_color(rgb(0xd9bf60)),
                                            )
                                            .child(
                                                div()
                                                    .text_color(rgb(MUTED))
                                                    .flex_none()
                                                    .child(pr_label(pr)),
                                            )
                                            .child(div().truncate().child(
                                                if pr.title.is_empty() {
                                                    "Pull request".into()
                                                } else {
                                                    pr.title.clone()
                                                },
                                            )),
                                    )
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(IssueEvent::OpenPullRequest(url.clone()))
                                    })),
                            )
                            .child(remove_link(
                                format!("unlink-pr-{}", pr.url),
                                "Unlink pull request",
                                remove,
                                cx,
                            )),
                    );
                }
            }
            if kind == 1 {
                for chat in &issue.threads {
                    let id = chat.clone();
                    let name = self.thread_title(&id);
                    values = values.child(
                        row()
                            .gap(px(2.))
                            .child(
                                button(format!("issue-thread-{id}"), "")
                                    .aria_label(SharedString::from(name.clone()))
                                    .flex_1()
                                    .min_w_0()
                                    .py(px(4.))
                                    .child(
                                        row()
                                            .min_w_0()
                                            .gap(px(6.))
                                            .child(icon("chat-round-line").size(px(12.)))
                                            .child(div().truncate().child(name)),
                                    )
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(IssueEvent::OpenThread(id.clone()))
                                    })),
                            )
                            .child(remove_link(
                                format!("unlink-thread-{chat}"),
                                "Unlink thread",
                                ZotAction::UnlinkIssueThread {
                                    issue_id: issue.id.clone(),
                                    chat_id: chat.clone(),
                                },
                                cx,
                            )),
                    );
                }
            }
            if kind == 2 {
                for id in &issue.details.relations {
                    if let Some(related) = self.zot.issues.iter().find(|i| i.id == *id) {
                        let target = id.clone();
                        let mut details = issue.details.clone();
                        details.relations.retain(|r| r != id);
                        values = values.child(
                            row()
                                .gap(px(2.))
                                .child(
                                    button(
                                        format!("issue-relation-{id}"),
                                        format!("{} {}", related.code(), related.details.title),
                                    )
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .py(px(4.))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.selected = Some(target.clone());
                                            this.menu = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(remove_link(
                                    format!("unlink-relation-{id}"),
                                    "Remove relation",
                                    ZotAction::EditIssue {
                                        issue_id: issue.id.clone(),
                                        details,
                                    },
                                    cx,
                                )),
                        );
                    }
                }
            }
            let id = issue.id.clone();
            values = values.child(
                button(
                    format!("issue-link-{kind}"),
                    format!(
                        "+ {}",
                        match kind {
                            0 => "Link pull request",
                            1 => "Link thread",
                            _ => "Add relation",
                        }
                    ),
                )
                .py(px(4.))
                .text_color(rgb(MUTED))
                .on_click(cx.listener(move |this, _, w, cx| {
                    if kind == 0 {
                        this.open(EditorKind::PullRequest(id.clone()), w, cx);
                    } else {
                        this.menu =
                            if this.menu == Some(if kind == 1 { "threads" } else { "relations" }) {
                                None
                            } else {
                                Some(if kind == 1 { "threads" } else { "relations" })
                            };
                        cx.notify();
                    }
                })),
            );
            let mut line = row()
                .relative()
                .items_start()
                .gap(px(8.))
                .text_size(px(12.))
                .line_height(px(18.))
                .py(px(0.))
                .child(icon(glyph).size(px(12.)).mt(px(6.)).text_color(rgb(MUTED)))
                .child(
                    div()
                        .w(px(84.))
                        .flex_none()
                        .pt(px(4.))
                        .text_color(rgb(MUTED))
                        .child(label),
                )
                .child(values);
            if (kind == 1 && self.menu == Some("threads"))
                || (kind == 2 && self.menu == Some("relations"))
            {
                let mut options = menu_card(format!("issue-links-{kind}"), 310.)
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }));
                if kind == 1 {
                    for worker in &self.zot.workers {
                        let id = issue.id.clone();
                        let chat = worker.chat_id.clone();
                        if issue.threads.contains(&chat) {
                            continue;
                        }
                        options = options.child(
                            menu_option(
                                format!("link-thread-{chat}"),
                                icon("chat-round-line")
                                    .size(px(13.))
                                    .text_color(rgb(MUTED))
                                    .into_any_element(),
                                self.thread_title(&chat),
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.menu = None;
                                    this.emit(
                                        ZotAction::LinkIssueThread {
                                            issue_id: id.clone(),
                                            chat_id: chat.clone(),
                                        },
                                        cx,
                                    );
                                },
                            )),
                        )
                    }
                    let id = issue.id.clone();
                    options = options.child(
                        menu_option(
                            "issue-start-thread",
                            icon("plus")
                                .size(px(13.))
                                .text_color(rgb(MUTED))
                                .into_any_element(),
                            "Start a new thread".into(),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.open(EditorKind::Delegate(id.clone()), w, cx)
                        })),
                    );
                } else {
                    for other in &self.zot.issues {
                        if other.id == issue.id || issue.details.relations.contains(&other.id) {
                            continue;
                        }
                        let mut details = issue.details.clone();
                        details.relations.push(other.id.clone());
                        let id = issue.id.clone();
                        options = options.child(
                            menu_option(
                                format!("link-relation-{}", other.id),
                                status_icon(other.details.status).into_any_element(),
                                format!("{}  {}", other.code(), other.details.title),
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.menu = None;
                                    this.emit(
                                        ZotAction::EditIssue {
                                            issue_id: id.clone(),
                                            details: details.clone(),
                                        },
                                        cx,
                                    );
                                },
                            )),
                        );
                    }
                }
                line = line.child(deferred(options).priority(3));
            }
            links = links.child(line);
        }
        let edit = issue.id.clone();
        let description = match self.editing(&issue.id, Field::Description) {
            Some(input) => inline_field(input, false, true, cx).into_any_element(),
            None => div()
                .id("issue-description")
                .role(Role::Button)
                .aria_label("Edit description")
                .mx(px(-6.))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(6.))
                .cursor_text()
                .hover(|d| d.bg(rgb(0x181818)))
                .text_size(px(13.))
                .line_height(px(19.))
                .when(issue.details.description.is_empty(), |d| {
                    d.text_color(rgb(0x6a6a6a))
                })
                .child(if issue.details.description.is_empty() {
                    "Add a description…".into()
                } else {
                    issue.details.description.clone()
                })
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.open(EditorKind::Field(edit.clone(), Field::Description), w, cx)
                }))
                .into_any_element(),
        };
        body = body
            .child(links)
            .child(
                column()
                    .pt(px(14.))
                    .gap(px(8.))
                    .child(div().text_color(rgb(MUTED)).child("Description"))
                    .child(description),
            )
            .child(self.text_section(
                issue,
                Field::DoneCriteria,
                "Done when",
                "Add what must be true for this to be done…",
                &issue.details.done_criteria,
                cx,
            ))
            .child(self.text_section(
                issue,
                Field::OutOfScope,
                "Out of scope",
                "Add what this issue leaves alone…",
                &issue.details.out_of_scope,
                cx,
            ));
        let mut filters = row().gap(px(6.));
        for (filter, name) in [(0, "All"), (1, "Comments"), (2, "Agents & bots")] {
            filters = filters.child(
                button(format!("issue-activity-filter-{filter}"), name)
                    .h(px(26.))
                    .min_w(px(48.))
                    .justify_center()
                    .px(px(12.))
                    .py(px(3.))
                    .text_size(px(11.))
                    .when(self.activity_filter == filter, |d| d.bg(rgb(0x232323)))
                    .when(self.activity_filter != filter, |d| d.text_color(rgb(MUTED)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activity_filter = filter;
                        cx.notify();
                    })),
            );
        }
        body = body.child(
            row()
                .pt(px(14.))
                .justify_between()
                .child(div().text_color(rgb(MUTED)).child("Activity"))
                .child(filters),
        );
        // Replies nest under the entry they answer.
        let mut replies: HashMap<&str, Vec<&ZotIssueActivity>> = HashMap::new();
        for event in &issue.activity {
            if let Some(parent) = &event.reply_to {
                replies.entry(parent.as_str()).or_default().push(event);
            }
        }
        for event in issue
            .activity
            .iter()
            .filter(|e| e.reply_to.is_none())
            .filter(|e| match self.activity_filter {
                1 => e.kind == "comment" || replies.contains_key(e.id.as_str()),
                2 => e.actor.is_some(),
                _ => true,
            })
        {
            let (actor, text) = self.activity_text(issue, event);
            let (glyph, color) = activity_glyph(event);
            if !matches!(event.kind.as_str(), "comment" | "workerReport" | "question") {
                // One line: glyph, who, what, when. Long text truncates
                // instead of wrapping, so every row keeps the same rhythm.
                body = body.child(
                    row()
                        .w_full()
                        .h(px(24.))
                        .gap(px(8.))
                        .text_size(px(12.))
                        .child(pixel_glyph(glyph, color))
                        .child(div().flex_none().text_color(rgb(0xb0b0b0)).child(actor))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(rgb(MUTED))
                                .child(text.replace('\n', " · ")),
                        )
                        .child(div().flex_none().text_color(rgb(0x4a4a4a)).child("·"))
                        .child(
                            div()
                                .flex_none()
                                .text_color(rgb(0x6a6a6a))
                                .child(relative_time(event.created_at)),
                        ),
                );
                continue;
            }
            let thread = replies
                .get(event.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            body = body.child(self.activity_card(
                issue,
                event,
                actor,
                text,
                (glyph, color),
                thread,
                cx,
            ));
        }
        let id = issue.id.clone();
        body.child(
            match self.inline_input(|k| matches!(k, EditorKind::Comment(i) if *i == issue.id)) {
                Some(input) => inline_composer(input, "Comment", cx)
                    .mt(px(6.))
                    .into_any_element(),
                None => button("issue-comment", "Leave a comment…")
                    .w_full()
                    .mt(px(6.))
                    .px(px(14.))
                    .py(px(11.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgb(0x262626))
                    .text_size(px(12.5))
                    .text_color(rgb(0x6a6a6a))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.open(EditorKind::Comment(id.clone()), w, cx)
                    }))
                    .into_any_element(),
            },
        )
        .into_any_element()
    }
    /// A titled block of issue text that edits in place on click.
    fn text_section(
        &self,
        issue: &ZotIssue,
        field: Field,
        label: &'static str,
        placeholder: &'static str,
        value: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = issue.id.clone();
        let content = match self.editing(&issue.id, field) {
            Some(input) => inline_field(input, false, true, cx).into_any_element(),
            None => div()
                .id(SharedString::from(format!("issue-{label}")))
                .role(Role::Button)
                .aria_label(SharedString::from(format!("Edit {label}")))
                .mx(px(-6.))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(6.))
                .cursor_text()
                .hover(|d| d.bg(rgb(0x181818)))
                .text_size(px(13.))
                .line_height(px(19.))
                .when(value.is_empty(), |d| d.text_color(rgb(0x6a6a6a)))
                .child(if value.is_empty() {
                    placeholder.to_owned()
                } else {
                    value.to_owned()
                })
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.open(EditorKind::Field(id.clone(), field), w, cx)
                }))
                .into_any_element(),
        };
        column()
            .pt(px(14.))
            .gap(px(8.))
            .child(div().text_color(rgb(MUTED)).child(label))
            .child(content)
    }
    /// A face for comments: this Zapper's, or the signed-in person's initial.
    /// Worker threads keep their activity glyph.
    fn actor_avatar(&self, event: &ZotIssueActivity, size: f32) -> Option<Div> {
        match &event.actor {
            None => Some(person_avatar(
                self.user_name.as_deref().unwrap_or("You"),
                size,
            )),
            Some(id)
                if *id == self.zot.coordinator_chat_id
                    || self.zot.previous_coordinator_chats.contains(id)
                    || *id == self.zot.config.name =>
            {
                Some(zapper_face(&self.avatar(), size))
            }
            Some(_) => None,
        }
    }
    /// Who did it and what, with chat IDs and PR URLs swapped for readable names.
    fn activity_text(&self, issue: &ZotIssue, event: &ZotIssueActivity) -> (String, String) {
        let actor = event
            .actor
            .as_ref()
            .map(|id| {
                if *id == self.zot.coordinator_chat_id
                    || self.zot.previous_coordinator_chats.contains(id)
                    || *id == self.zot.config.name
                {
                    self.zot.config.name.clone()
                } else if self.zot.workers.iter().any(|w| w.chat_id == *id)
                    || self.thread_titles.contains_key(id)
                {
                    self.thread_title(id)
                } else {
                    id.clone()
                }
            })
            .unwrap_or_else(|| "You".into());
        let mut text = event.text.clone();
        // Older reports began "Thread <chat id> · <kind>"; the actor already
        // names the thread, so keep only the report itself.
        if event.kind == "workerReport" {
            if let Some((first, rest)) = text.split_once('\n') {
                if first.starts_with("Thread ") && first.contains(" · ") {
                    text = rest.to_owned();
                }
            }
        }
        // Longest IDs first, whole IDs only, so "issue-worker" never
        // rewrites the start of "issue-worker-attribution".
        let mut workers: Vec<_> = self.zot.workers.iter().map(|w| &w.chat_id).collect();
        workers.sort_by_key(|id| std::cmp::Reverse(id.len()));
        for id in workers {
            text = replace_whole_id(&text, id, &self.thread_title(id));
        }
        for pr in &issue.pull_requests {
            text = text.replace(&pr.url, &pr_label(pr));
        }
        (actor, text)
    }
    /// A comment or worker report, its replies, and an inline reply box.
    #[allow(clippy::too_many_arguments)]
    fn activity_card(
        &self,
        issue: &ZotIssue,
        event: &ZotIssueActivity,
        actor: String,
        mut text: String,
        (glyph, color): (&'static [&'static str], u32),
        thread: &[&ZotIssueActivity],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A routine "Worker update" label repeats the header; failures and
        // questions stay visible as a tag.
        let mut tag = (event.kind == "question").then(|| {
            if issue.details.status == ZotIssueStatus::NeedsYou {
                "Needs your answer".to_owned()
            } else {
                "Question".to_owned()
            }
        });
        if event.kind == "workerReport" {
            if let Some((first, rest)) = text.split_once('\n') {
                if first == "Worker update" {
                    text = rest.to_owned();
                } else if first.starts_with("Worker ") {
                    tag = Some(first.trim_start_matches("Worker ").to_owned());
                    text = rest.to_owned();
                }
            }
        }
        let text = text.trim().to_owned();
        const PREVIEW: usize = 480;
        let long = text.chars().count() > PREVIEW;
        let open = self.expanded.contains(&event.id);
        let shown = if long && !open {
            format!(
                "{}…",
                text.chars().take(PREVIEW).collect::<String>().trim_end()
            )
        } else {
            text.clone()
        };
        let key = event.id.clone();
        let (issue_id, parent) = (issue.id.clone(), event.id.clone());
        let replying = self.inline_input(
            |k| matches!(k, EditorKind::Reply(i, p) if *i == issue.id && *p == event.id),
        );
        let lead = match self.actor_avatar(event, 18.) {
            Some(avatar) => avatar.into_any_element(),
            None => pixel_glyph(glyph, color).into_any_element(),
        };
        let header = row()
            .gap(px(8.))
            .text_size(px(12.))
            .line_height(px(16.))
            .child(lead)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(0xdedede))
                    .child(actor),
            )
            .when_some(tag, |d, tag| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded(px(4.))
                        .bg(rgb(0x2a2414))
                        .text_size(px(11.))
                        .text_color(rgb(0xd9bf60))
                        .child(tag),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(0x6a6a6a))
                    .child(relative_time(event.created_at)),
            )
            .child(div().flex_1())
            .child(
                button(format!("issue-copy-{key}"), "")
                    .aria_label("Copy")
                    .p(px(4.))
                    .text_color(rgb(0x6a6a6a))
                    .child(icon("copy").size(px(12.)))
                    .on_click({
                        let text = text.clone();
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
                        }
                    }),
            )
            .child(
                button(format!("issue-reply-{key}"), "")
                    .aria_label("Reply")
                    .p(px(4.))
                    .text_color(rgb(0x6a6a6a))
                    .child(icon("return").size(px(12.)))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.open(EditorKind::Reply(issue_id.clone(), parent.clone()), w, cx)
                    })),
            );
        let mut card = column()
            .w_full()
            .my(px(4.))
            .rounded(px(10.))
            .bg(rgb(0x141414))
            .border_1()
            .border_color(rgb(0x222222))
            .child(
                column()
                    .px(px(14.))
                    .pt(px(10.))
                    .pb(px(12.))
                    .gap(px(6.))
                    .child(header)
                    .child(
                        div()
                            .text_size(px(12.5))
                            .line_height(px(19.))
                            .text_color(rgb(0xc8c8c8))
                            .child(shown),
                    )
                    .when(long, |d| {
                        let key = key.clone();
                        d.child(
                            div()
                                .id(SharedString::from(format!("issue-activity-more-{key}")))
                                .role(Role::Button)
                                .cursor_pointer()
                                .text_size(px(12.))
                                .text_color(rgb(MUTED))
                                .hover(|d| d.text_color(rgb(TEXT)))
                                .child(if open { "Show less" } else { "Show more" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.expanded.remove(&key) {
                                        this.expanded.insert(key.clone());
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            );
        for reply in thread {
            let (who, said) = self.activity_text(issue, reply);
            card = card.child(
                column()
                    .px(px(14.))
                    .py(px(10.))
                    .gap(px(4.))
                    .border_t_1()
                    .border_color(rgb(0x1e1e1e))
                    .child(
                        row()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .child(match self.actor_avatar(reply, 16.) {
                                Some(avatar) => avatar.into_any_element(),
                                None => pixel_glyph(GLYPH_REPLY, 0x6f7a8a).into_any_element(),
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(0xcfcfcf))
                                    .child(who),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(rgb(0x6a6a6a))
                                    .child(relative_time(reply.created_at)),
                            ),
                    )
                    .child(
                        div()
                            .pl(px(20.))
                            .text_size(px(12.5))
                            .line_height(px(19.))
                            .text_color(rgb(0xbdbdbd))
                            .child(said.trim().to_owned()),
                    ),
            );
        }
        if let Some(input) = replying {
            card = card.child(
                div()
                    .px(px(10.))
                    .pt(px(2.))
                    .pb(px(10.))
                    .child(inline_composer(input, "Reply", cx)),
            );
        }
        card.into_any_element()
    }
    /// The open inline editor's input, when it matches.
    fn inline_input(&self, matches: impl Fn(&EditorKind) -> bool) -> Option<Entity<TextInput>> {
        self.editor
            .as_ref()
            .filter(|e| matches(&e.kind))
            .and_then(|e| e.fields.first().cloned())
    }
    fn editing(&self, issue: &str, field: Field) -> Option<Entity<TextInput>> {
        self.inline_input(|k| matches!(k, EditorKind::Field(i, f) if i == issue && *f == field))
    }
    fn editor_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let editor = self.editor.as_ref().unwrap();
        let title = match editor.kind {
            EditorKind::Create => "New issue",
            EditorKind::Field(..) | EditorKind::Comment(_) | EditorKind::Reply(..) => "",
            EditorKind::PullRequest(_) => "Link pull request",
            EditorKind::Delegate(_) => "Start issue thread",
        };
        let mut body = column()
            .gap(px(14.))
            .child(div().text_size(px(20.)).child(title));
        let labels: &[&str] = match editor.kind {
            EditorKind::Create => &["Title", "Description", "Done when"],
            EditorKind::Field(..) | EditorKind::Comment(_) | EditorKind::Reply(..) => &[],
            EditorKind::PullRequest(_) => &["Pull request URL", "Title"],
            EditorKind::Delegate(_) => &["Assignment", "Base branch"],
        };
        for (index, input) in editor.fields.iter().enumerate() {
            let multiline = matches!(editor.kind, EditorKind::Create) && index >= 1
                || matches!(editor.kind, EditorKind::Delegate(_)) && index == 0;
            body = body.child(
                column()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(labels[index]),
                    )
                    .child(input_box(input.clone(), if multiline { 120. } else { 36. })),
            );
        }
        if matches!(editor.kind, EditorKind::Delegate(_)) {
            body=body.child(div().text_color(rgb(MUTED)).text_size(px(12.)).child("Assign this issue to the Zapper first. The thread starts in the repository selected below."));
            // Put the chosen repository first without changing the Zot config.
            for repo in &self.zot.config.repositories {
                let selected = self
                    .repository
                    .as_ref()
                    .or(self.zot.config.repositories.first())
                    == Some(repo);
                let r = repo.clone();
                body = body.child(
                    button(
                        format!("issue-repo-{}", repo.space_id),
                        format!("{}{}", if selected { "✓ " } else { "" }, repo.space_id),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.repository = Some(r.clone());
                        cx.notify();
                    })),
                );
            }
        }
        body = body.child(
            row()
                .justify_end()
                .gap(px(8.))
                .child(
                    button("issue-editor-cancel", "Cancel").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.editor = None;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    button(
                        "issue-editor-save",
                        if self.pending { "Saving…" } else { "Save" },
                    )
                    .bg(rgb(0x232323))
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
        );
        body.into_any_element()
    }
    /// Called by the shell when a command succeeds; a failed write preserves the draft.
    pub fn committed(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.menu = None;
        cx.notify();
    }
}
impl Render for IssuePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.search.is_none() {
            let search = cx.new(|cx| TextInput::new("Search issues…", window, cx));
            cx.observe(&search, |_, _, cx| cx.notify()).detach();
            self.search = Some(search);
        }
        // Inline editors (fields, comments, replies) live inside the issue page.
        let body = if self.editor.as_ref().is_some_and(|e| !e.kind.inline()) {
            self.editor_view(cx)
        } else if let Some(issue) = self
            .selected
            .as_ref()
            .and_then(|id| self.zot.issues.iter().find(|i| i.id == *id))
            .cloned()
        {
            self.detail(&issue, cx)
        } else {
            self.list(cx)
        };
        column()
            .w_full()
            .min_w_0()
            .gap(px(12.))
            .text_size(px(13.))
            .text_color(rgb(TEXT))
            .child(body)
            .when_some(self.error.clone(), |d, e| {
                d.child(div().text_color(rgb(0xe5a0a0)).child(e))
            })
    }
}
fn row() -> Div {
    div().flex().items_center()
}
fn column() -> Div {
    div().flex().flex_col()
}
/// A clickable control with no hover style, for callers that set their own.
fn plain_button(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.clone())
        .flex()
        .items_center()
        .line_height(px(18.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
        .child(label)
}
fn button(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id.into())
        .role(Role::Button)
        .aria_label(label.clone())
        .flex()
        .items_center()
        .line_height(px(18.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|d| d.bg(rgb(0x232323)))
        .child(label)
}
/// A status group's header: its icon, name and count, folding on click.
fn section(
    status: ZotIssueStatus,
    count: usize,
    key: &str,
    closed: bool,
    cx: &Context<IssuePanel>,
) -> Stateful<Div> {
    let key = key.to_owned();
    row()
        .id(SharedString::from(format!("issue-group-{key}")))
        .role(Role::Button)
        .aria_label(SharedString::from(format!("{} {count}", status.label())))
        .aria_expanded(!closed)
        .w_full()
        .h(px(30.))
        .mt(px(6.))
        .px(px(10.))
        .gap(px(8.))
        .rounded(px(6.))
        .bg(rgb(0x151515))
        .cursor_pointer()
        .text_size(px(12.))
        .child(status_icon(status))
        .child(div().text_color(rgb(0xa4a8a3)).child(status.label()))
        .child(div().text_color(rgb(FAINT)).child(count.to_string()))
        .child(div().flex_1())
        .child(
            icon(if closed {
                "alt-arrow-right"
            } else {
                "alt-arrow-down"
            })
            .size(px(11.))
            .text_color(rgb(FAINT)),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.collapsed.remove(&key) {
                this.collapsed.insert(key.clone());
            }
            cx.notify();
        }))
}
/// A dropdown under a property: dark card, small radius, soft shadow.
fn menu_card(id: impl Into<SharedString>, width: f32) -> Stateful<Div> {
    column()
        .id(id.into())
        .absolute()
        .left(px(100.))
        .top(px(30.))
        .w(px(width))
        .occlude()
        .p(px(4.))
        .gap(px(1.))
        .rounded(px(8.))
        .bg(rgb(MENU))
        .border_1()
        .border_color(rgb(0x2a2a2a))
        .shadow_lg()
}
/// One menu choice: its picture, label and a check on the current one.
fn menu_option(
    id: impl Into<SharedString>,
    lead: AnyElement,
    label: String,
    current: bool,
) -> Stateful<Div> {
    row()
        .id(id.into())
        .role(Role::MenuItem)
        .aria_label(SharedString::from(label.clone()))
        .w_full()
        .h(px(30.))
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(5.))
        .cursor_pointer()
        .text_size(px(12.5))
        .hover(|d| d.bg(rgb(0x2a2a2a)))
        .child(
            div()
                .size(px(18.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(lead),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when(current, |d| {
            d.child(icon("check").size(px(12.)).text_color(rgb(TEXT)))
        })
}
fn input_box(input: Entity<TextInput>, height: f32) -> Div {
    div()
        .w_full()
        .min_h(px(height))
        .p(px(10.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(BORDER))
        .child(input)
}
/// The Needs-you amber, shared by its status, question cards and badges.
pub const NEEDS_YOU: u32 = 0xf2a14a;
pub fn status_color(status: ZotIssueStatus) -> u32 {
    match status {
        ZotIssueStatus::NeedsYou => NEEDS_YOU,
        ZotIssueStatus::InProgress | ZotIssueStatus::InReview | ZotIssueStatus::Done => 0x00b80a,
        ZotIssueStatus::Blocked => 0xd9bf60,
        _ => MUTED,
    }
}
pub fn status_icon(status: ZotIssueStatus) -> Div {
    let color = status_color(status);
    div()
        .size(px(12.))
        .rounded_full()
        .border_2()
        .border_color(rgb(color))
        .when(
            matches!(
                status,
                ZotIssueStatus::InProgress
                    | ZotIssueStatus::InReview
                    | ZotIssueStatus::Done
                    | ZotIssueStatus::NeedsYou
            ),
            |d| d.child(div().size(px(5.)).bg(rgb(color))),
        )
}
fn pr_badge(pr: &ZotIssuePullRequest) -> Div {
    row()
        .flex_none()
        .gap(px(4.))
        .h(px(20.))
        .px(px(6.))
        .rounded(px(5.))
        .bg(rgb(0x24251f))
        .text_size(px(11.5))
        .text_color(rgb(TEXT))
        .child(icon("git-branch").size(px(11.)).text_color(rgb(0xd9bf60)))
        .child(format!("#{}", pr.url.rsplit('/').next().unwrap_or("")))
}
/// A report sent back to the Zot, shown like a chat-to-chat reply: a quiet
/// "Sent by <thread>" bubble. The model-facing evidence label stays hidden.
pub fn activity_notice(
    sender: &zuse_protocol::presentation::zot_message::ZotSender,
    body: &str,
    timestamp: i64,
) -> Div {
    let mut sender = sender.clone();
    // Notices saved before the source was recorded read "<Zot> · Activity".
    if let Some(name) = sender.name.strip_suffix(" · Activity") {
        sender.name = name.to_owned();
    }
    let text = match body.split_once('\n') {
        Some((first, rest)) if first.starts_with("Activity update") => rest,
        _ => body,
    }
    .trim()
    .to_owned();
    // Worker reports can be long; the copy action keeps the full text.
    const SHOWN: usize = 1200;
    let shown = if text.chars().count() > SHOWN {
        format!(
            "{}…",
            text.chars().take(SHOWN).collect::<String>().trim_end()
        )
    } else {
        text.clone()
    };
    column()
        .w_full()
        .child(
            div()
                .w_full()
                .px(px(14.))
                .py(px(10.))
                .rounded(px(12.))
                .bg(rgb(0x1d1d1d))
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(rgb(TEXT))
                .child(sender_header(&sender, timestamp))
                .child(shown),
        )
        .child(sender_footer(text, timestamp))
}

/// Saved Zot identity above an agent-authored message, shared by both clients.
pub fn sender_header(
    sender: &zuse_protocol::presentation::zot_message::ZotSender,
    timestamp: i64,
) -> Div {
    let _ = timestamp;
    sender_header_labeled(sender, &format!("Sent by {}", sender.name))
}

/// The same header with its own wording, such as "Sent from Zapper".
pub fn sender_header_labeled(
    sender: &zuse_protocol::presentation::zot_message::ZotSender,
    label: &str,
) -> Div {
    // The Zapper's own messages carry its face; relayed thread activity
    // keeps the terminal mark so the two never read as the same author.
    let lead = match sender.avatar.as_ref().filter(|_| !sender.activity) {
        Some(avatar) => zapper_face(&known_face(&sender.zot_id, avatar), 14.).into_any_element(),
        None => icon("terminal").size(px(12.)).into_any_element(),
    };
    row()
        .gap(px(6.))
        .pb(px(5.))
        .text_size(px(12.))
        .text_color(rgb(0x959595))
        .child(lead)
        .child(label.to_owned())
}

pub fn sender_footer(body: String, timestamp: i64) -> Div {
    let time = chrono::DateTime::from_timestamp_millis(timestamp)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%-I:%M %p")
                .to_string()
        })
        .unwrap_or_default();
    row()
        .justify_end()
        .gap(px(10.))
        .pt(px(6.))
        .text_size(px(11.))
        .text_color(rgb(0x858585))
        .child(
            div()
                .id(SharedString::from(format!("copy-zot-message-{timestamp}")))
                .role(Role::Button)
                .aria_label("Copy message")
                .cursor_pointer()
                .child(icon("copy").size(px(12.)))
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(body.clone()))
                }),
        )
        .child(time)
}

fn remove_link(
    id: impl Into<SharedString>,
    label: &'static str,
    action: ZotAction,
    cx: &Context<IssuePanel>,
) -> Stateful<Div> {
    button(id, "")
        .aria_label(label)
        .px(px(4.))
        .py(px(4.))
        .text_color(rgb(MUTED))
        .child(icon("close").size(px(11.)))
        .on_click(cx.listener(move |this, _, _, cx| this.emit(action.clone(), cx)))
}
/// A property edited where it sits: the field, then a quiet save/cancel line.
fn inline_field(
    input: Entity<TextInput>,
    large: bool,
    multiline: bool,
    cx: &Context<IssuePanel>,
) -> Div {
    column()
        .w_full()
        .gap(px(4.))
        .child(
            div()
                .w_full()
                .mx(px(-6.))
                .px(px(6.))
                .py(px(if large { 2. } else { 1. }))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(0x363636))
                .bg(rgb(0x121212))
                .child(input),
        )
        .child(inline_actions("Save", multiline, cx))
}
/// A comment or reply box with its send action inside the frame.
fn inline_composer(input: Entity<TextInput>, verb: &'static str, cx: &Context<IssuePanel>) -> Div {
    column()
        .w_full()
        .gap(px(4.))
        .px(px(10.))
        .pt(px(4.))
        .pb(px(8.))
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(0x2e2e2e))
        .bg(rgb(0x121212))
        .child(input)
        .child(inline_actions(verb, true, cx))
}
fn inline_actions(verb: &'static str, multiline: bool, cx: &Context<IssuePanel>) -> Div {
    row()
        .gap(px(4.))
        .text_size(px(11.))
        .child(
            div()
                .flex_1()
                .text_color(rgb(0x555555))
                .child(if multiline {
                    "Enter to save · Shift+Enter for a new line"
                } else {
                    "Enter to save"
                }),
        )
        .child(
            button("issue-inline-cancel", "Cancel")
                .py(px(3.))
                .text_color(rgb(MUTED))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.editor = None;
                    this.error = None;
                    cx.notify();
                })),
        )
        .child(
            button("issue-inline-save", verb)
                .py(px(3.))
                .bg(rgb(0x262626))
                .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
        )
}

// Activity glyphs: tiny 5×5 pixel marks, drawn rather than loaded, for a
// quiet retro feel next to the regular line icons.
const GLYPH_PLUS: &[&str] = &["..#..", "..#..", "#####", "..#..", "..#.."];
const GLYPH_DOT: &[&str] = &[".###.", "#####", "#####", "#####", ".###."];
const GLYPH_BRANCH: &[&str] = &["#...#", "#...#", "#..#.", "#.#..", "##..."];
const GLYPH_CHAT: &[&str] = &["#####", "#...#", "#...#", "#####", ".#..."];
const GLYPH_PERSON: &[&str] = &[".###.", ".###.", "..#..", "#####", "#...#"];
const GLYPH_PROMPT: &[&str] = &["#....", ".#...", "..#..", ".#...", "#.###"];
const GLYPH_BARS: &[&str] = &["....#", "..#.#", "..#.#", "#.#.#", "#.#.#"];
const GLYPH_PENCIL: &[&str] = &["...##", "..##.", ".##..", "##...", "#...."];
const GLYPH_REPLY: &[&str] = &["#....", "#....", "#...#", "#####", "....#"];

fn activity_glyph(event: &ZotIssueActivity) -> (&'static [&'static str], u32) {
    let text = event.text.as_str();
    match event.kind.as_str() {
        "created" => (GLYPH_PLUS, 0x8a8a8a),
        "pullRequest" if text.starts_with("Unlinked") => (GLYPH_BRANCH, 0x6a6a6a),
        "pullRequest" => (GLYPH_BRANCH, 0xd9bf60),
        "thread" if text.starts_with("Unlinked") => (GLYPH_CHAT, 0x6a6a6a),
        "thread" => (GLYPH_CHAT, 0x7aa2f7),
        "comment" => (GLYPH_CHAT, 0x9aa3b2),
        "question" => (GLYPH_PERSON, NEEDS_YOU),
        "closed" if text.starts_with("Merged") => (GLYPH_DOT, 0x00b80a),
        "closed" => (GLYPH_DOT, 0x6a6a6a),
        "workerReport" if text.starts_with("Worker failed") => (GLYPH_PROMPT, 0xe5736b),
        "workerReport" if text.starts_with("Worker needs input") => (GLYPH_PROMPT, 0xd9bf60),
        "workerReport" => (GLYPH_PROMPT, 0x4fc1b0),
        "updated" if text.starts_with("Status changed to ") => {
            let status = ZotIssueStatus::ALL
                .iter()
                .find(|s| text["Status changed to ".len()..].starts_with(s.label()))
                .copied()
                .unwrap_or(ZotIssueStatus::Todo);
            (GLYPH_DOT, status_color(status))
        }
        "updated" if text.starts_with("Priority") => (GLYPH_BARS, 0xe0965a),
        "updated" if text.starts_with("Assigned") || text.starts_with("Removed assignee") => {
            (GLYPH_PERSON, 0xa78bfa)
        }
        _ => (GLYPH_PENCIL, 0x8a8a8a),
    }
}
fn pixel_glyph(rows: &'static [&'static str], color: u32) -> Div {
    const CELL: f32 = 2.;
    div().size(px(12.)).flex_none().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0) as f32;
                let x0 = bounds.origin.x + (bounds.size.width - px(cols * CELL)) / 2.;
                let y0 = bounds.origin.y + (bounds.size.height - px(rows.len() as f32 * CELL)) / 2.;
                for (y, line) in rows.iter().enumerate() {
                    for (x, c) in line.bytes().enumerate() {
                        if c == b'#' {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(x0 + px(x as f32 * CELL), y0 + px(y as f32 * CELL)),
                                    size(px(CELL), px(CELL)),
                                ),
                                rgb(color),
                            ));
                        }
                    }
                }
            },
        )
        .size(px(12.)),
    )
}
/// Replaces `id` only where it stands alone, not inside a longer ID.
fn replace_whole_id(text: &str, id: &str, with: &str) -> String {
    if id.is_empty() {
        return text.to_owned();
    }
    let part = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(id) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + id.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_some_and(part) || after.is_some_and(part) {
            out.push_str(id);
        } else {
            out.push_str(with);
        }
        rest = &rest[at + id.len()..];
    }
    out.push_str(rest);
    out
}
fn pr_label(pr: &ZotIssuePullRequest) -> String {
    let parts: Vec<_> = pr.url.trim_end_matches('/').rsplit('/').collect();
    format!(
        "{}#{}",
        parts.get(2).unwrap_or(&""),
        parts.first().unwrap_or(&"")
    )
}
fn priority_icon(priority: ZotIssuePriority) -> Div {
    let count = match priority {
        ZotIssuePriority::Low => 1,
        ZotIssuePriority::Normal => 2,
        _ => 3,
    };
    let mut bars = row().items_end().h(px(12.)).gap(px(2.));
    for i in 0..3 {
        bars = bars.child(
            div()
                .w(px(2.5))
                .h(px(4. + i as f32 * 3.))
                .rounded(px(1.))
                .bg(rgb(if i < count { 0x929292 } else { 0x343434 })),
        );
    }
    bars
}
/// "now", "5m", "3h", "2d": the row's narrow time column.
fn short_time(millis: i64) -> String {
    let seconds = (chrono::Utc::now().timestamp_millis() - millis).max(0) / 1000;
    match seconds {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s if s < 86400 * 7 => format!("{}d", s / 86400),
        s => format!("{}w", s / (86400 * 7)),
    }
}
fn relative_time(millis: i64) -> String {
    let seconds = (chrono::Utc::now().timestamp_millis() - millis).max(0) / 1000;
    if seconds < 60 {
        "Just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    }
}
