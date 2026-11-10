//! The same issue surface is embedded by the browser and native Zot inspectors.
use crate::{assets::icon, input::TextInput, palette::rgb};
use gpui::{prelude::*, *};
use std::collections::{HashMap, HashSet};
use zuse_protocol::zots::*;

const TEXT: u32 = 0xe5e5e5;
const MUTED: u32 = 0x777777;
const SURFACE: u32 = 0x191919;
const BORDER: u32 = 0x343434;
#[derive(Clone)]
pub enum IssueEvent {
    Command(ZotAction, u32),
    OpenThread(String),
    OpenDocument(ZotFile),
    OpenPullRequest(String),
}
pub struct IssuePanel {
    zot: Zot,
    thread_titles: HashMap<String, String>,
    overview: bool,
    selected: Option<String>,
    assigned_only: bool,
    mentioned_only: bool,
    collapsed: HashSet<String>,
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
            assigned_only: false,
            mentioned_only: false,
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
            if self.zot.id != zot.id || self.overview != overview {
                self.selected = None;
                self.editor = None;
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
        row()
            .id(SharedString::from(format!("issue-row-{id}")))
            .role(Role::Button)
            .aria_label(SharedString::from(format!(
                "{} {}",
                issue.code(),
                issue.details.title
            )))
            .w_full()
            .gap(px(10.))
            .px(px(10.))
            .py(px(if self.overview { 6. } else { 8. }))
            .line_height(px(20.))
            .rounded(px(7.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x232323)))
            .child(status_icon(issue.details.status))
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(div().truncate().child(issue.details.title.clone()))
                    .when(self.overview, |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_color(rgb(MUTED))
                                .text_size(px(12.))
                                .child(format!(
                                    "{}{}",
                                    issue.code(),
                                    if issue.threads.is_empty() {
                                        String::new()
                                    } else {
                                        format!(
                                            "  with {} thread{}",
                                            issue.threads.len(),
                                            if issue.threads.len() == 1 { "" } else { "s" }
                                        )
                                    }
                                )),
                        )
                    }),
            )
            .when_some(issue.pull_requests.first(), |d, pr| d.child(pr_badge(pr)))
            .child(priority_icon(issue.details.priority))
            .child(icon("chat-round-line").size(px(12.)).text_color(rgb(MUTED)))
            .when(!self.overview, |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(relative_time(issue.updated_at)),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(id.clone());
                this.editor = None;
                this.menu = None;
                cx.notify();
            }))
            .into_any_element()
    }
    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = column()
            .gap(px(12.))
            .when(!self.overview, |d| d.mt(px(-12.)));
        if !self.overview {
            let mut toolbar = row().gap(px(6.));
            for (index, label) in [(0, "All"), (1, "Assigned to Zapper"), (2, "Mentioned")] {
                let selected = match index {
                    1 => self.assigned_only,
                    2 => self.mentioned_only,
                    _ => !self.assigned_only && !self.mentioned_only,
                };
                toolbar = toolbar.child(
                    button(format!("issues-filter-{index}"), label)
                        .border_1()
                        .border_color(rgb(BORDER))
                        .py(px(6.))
                        .text_size(px(12.))
                        .when(selected, |d| d.bg(rgb(0x232323)))
                        .when(!selected, |d| d.text_color(rgb(MUTED)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.assigned_only = index == 1;
                            this.mentioned_only = index == 2;
                            cx.notify();
                        })),
                );
            }
            body = body.child(
                toolbar
                    .child(div().flex_1())
                    .child(
                        button("issues-search", "")
                            .aria_label("Search issues")
                            .child(icon("magnifer").size(px(12.)))
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.searching = !this.searching;
                                if this.searching {
                                    if let Some(input) = &this.search {
                                        input.update(cx, |i, cx| i.focus(cx));
                                    }
                                }
                                let _ = w;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("issues-create", "")
                            .aria_label("New issue")
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(icon("plus").size(px(12.)))
                            .on_click(
                                cx.listener(|this, _, w, cx| this.open(EditorKind::Create, w, cx)),
                            ),
                    ),
            );
            if self.searching {
                if let Some(search) = &self.search {
                    body = body.child(input_box(search.clone(), 36.));
                }
            }
        }
        let query = self
            .search
            .as_ref()
            .filter(|_| self.searching)
            .map(|i| i.read(cx).value(cx).to_lowercase())
            .unwrap_or_default();
        for status in ZotIssueStatus::ALL {
            let issues: Vec<_> = self
                .zot
                .issues
                .iter()
                .filter(|i| {
                    i.details.status == status
                        && (self.overview || !self.assigned_only || i.details.assigned)
                        && (self.overview
                            || !self.mentioned_only
                            || format!(
                                "{} {} {}",
                                i.details.title,
                                i.details.description,
                                i.activity
                                    .iter()
                                    .map(|a| a.text.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            )
                            .to_lowercase()
                            .contains(&format!("@{}", self.zot.config.name.to_lowercase())))
                        && (query.is_empty()
                            || format!(
                                "{} {} {}",
                                i.code(),
                                i.details.title,
                                i.details.description
                            )
                            .to_lowercase()
                            .contains(&query))
                })
                .collect();
            if issues.is_empty()
                && !matches!(status, ZotIssueStatus::Done | ZotIssueStatus::Canceled)
            {
                continue;
            }
            if self.overview && issues.is_empty() {
                continue;
            }
            let key = status.label().to_owned();
            let closed = self.collapsed.contains(&key);
            body = body.child(section(
                if closed && issues.is_empty() {
                    status.label().to_owned()
                } else {
                    format!("{}  {}", status.label(), issues.len())
                },
                &key,
                closed,
                cx,
            ));
            if !closed {
                for i in issues {
                    body = body.child(self.issue_row(i, cx));
                }
            }
        }
        if self.zot.issues.is_empty() {
            body=body.child(div().px(px(10.)).py(px(24.)).text_color(rgb(MUTED)).child("Create an issue, assign it to this Zapper, and track its threads and pull requests here."));
        }
        if self.overview {
            body = body.child(group_header(format!("Threads  {}", self.zot.workers.len())));
            for worker in &self.zot.workers {
                let id = worker.chat_id.clone();
                body = body.child(
                    row()
                        .id(SharedString::from(format!("overview-thread-{id}")))
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgb(0x232323)))
                        .child(icon("chat-round-line").size(px(16.)))
                        .child(
                            column()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().child(worker.objective.clone()))
                                .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child({
                                    let codes = self
                                        .zot
                                        .issues
                                        .iter()
                                        .filter(|i| i.threads.contains(&id))
                                        .map(ZotIssue::code)
                                        .collect::<Vec<_>>()
                                        .join(" · ");
                                    if codes.is_empty() {
                                        "Zapper thread".to_owned()
                                    } else {
                                        codes
                                    }
                                })),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(IssueEvent::OpenThread(id.clone()))
                        })),
                );
            }
            let prs: Vec<_> = self
                .zot
                .issues
                .iter()
                .flat_map(|i| i.pull_requests.iter())
                .collect();
            body = body.child(group_header(format!("Pull requests  {}", prs.len())));
            for pr in prs {
                let url = pr.url.clone();
                body = body.child(
                    row()
                        .id(SharedString::from(format!("overview-pr-{url}")))
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(8.))
                        .cursor_pointer()
                        .child(pr_badge(pr))
                        .child(div().truncate().child(pr.title.clone()))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(IssueEvent::OpenPullRequest(url.clone()))
                        })),
                );
            }
        }
        if self.overview {
            let files: Vec<_> = self
                .files
                .iter()
                .filter(|f| f.path != "INSTRUCTIONS.md" && !f.content.trim().is_empty())
                .collect();
            body = body.child(group_header(format!("Drive  {}", files.len())));
            let mut cards = row().items_start().flex_wrap().gap(px(10.));
            for file in files {
                let f = file.clone();
                cards = cards.child(
                    column()
                        .id(SharedString::from(format!("issue-document-{}", file.path)))
                        .w(px(160.))
                        .gap(px(8.))
                        .cursor_pointer()
                        .child(
                            div()
                                .h(px(180.))
                                .p(px(12.))
                                .overflow_hidden()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .text_size(px(12.))
                                .line_height(px(17.))
                                .child(file.content.chars().take(220).collect::<String>()),
                        )
                        .child(div().truncate().child(file.path.clone()))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(IssueEvent::OpenDocument(f.clone()))
                        })),
                );
            }
            body = body.child(cards);
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
                issue.details.status.label().to_owned(),
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
                "Assignee",
                if issue.details.assigned {
                    self.zot.config.name.clone()
                } else {
                    "No assignee".into()
                },
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
            if menu == "assignee" && issue.details.assigned {
                content = content.child(icon("bot").size(px(12.)));
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
                let mut options = column()
                    .id(SharedString::from(format!("issue-menu-{menu}")))
                    .absolute()
                    .left(px(106.))
                    .top(px(28.))
                    .w(px(250.))
                    .occlude()
                    .p(px(6.))
                    .rounded(px(8.))
                    .bg(rgb(0x232323))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }));
                let labels: Vec<_> = match menu {
                    "status" => ZotIssueStatus::ALL.iter().map(|s| s.label()).collect(),
                    "priority" => ZotIssuePriority::ALL.iter().map(|s| s.label()).collect(),
                    _ => vec!["No assignee", self.zot.config.name.as_str()],
                };
                for (index, label) in labels.into_iter().enumerate() {
                    let mut details = issue.details.clone();
                    match menu {
                        "status" => details.status = ZotIssueStatus::ALL[index],
                        "priority" => details.priority = ZotIssuePriority::ALL[index],
                        _ => details.assigned = index == 1,
                    };
                    let id = issue.id.clone();
                    options = options.child(
                        button(format!("issue-option-{menu}-{index}"), label.to_owned())
                            .w_full()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
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
                let mut options = column()
                    .id(SharedString::from(format!("issue-links-{kind}")))
                    .absolute()
                    .left(px(106.))
                    .top(px(28.))
                    .w(px(310.))
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .occlude()
                    .p(px(6.))
                    .rounded(px(8.))
                    .bg(rgb(0x232323))
                    .border_1()
                    .border_color(rgb(BORDER))
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
                            button(format!("link-thread-{chat}"), worker.objective.clone())
                                .truncate()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.menu = None;
                                    this.emit(
                                        ZotAction::LinkIssueThread {
                                            issue_id: id.clone(),
                                            chat_id: chat.clone(),
                                        },
                                        cx,
                                    );
                                })),
                        )
                    }
                    let id = issue.id.clone();
                    options = options.child(
                        button("issue-start-thread", "+ Start a new thread").on_click(cx.listener(
                            move |this, _, w, cx| {
                                this.open(EditorKind::Delegate(id.clone()), w, cx)
                            },
                        )),
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
                            button(
                                format!("link-relation-{}", other.id),
                                format!("{} {}", other.code(), other.details.title),
                            )
                            .truncate()
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
        body = body.child(links).child(
            column()
                .pt(px(14.))
                .gap(px(8.))
                .child(div().text_color(rgb(MUTED)).child("Description"))
                .child(description),
        );
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
            if event.kind != "comment" && event.kind != "workerReport" {
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
        let mut tag = None;
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
        let header = row()
            .gap(px(8.))
            .text_size(px(12.))
            .line_height(px(16.))
            .child(pixel_glyph(glyph, color))
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
                            .child(pixel_glyph(GLYPH_REPLY, 0x6f7a8a))
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
            EditorKind::Create => &["Title", "Description"],
            EditorKind::Field(..) | EditorKind::Comment(_) | EditorKind::Reply(..) => &[],
            EditorKind::PullRequest(_) => &["Pull request URL", "Title"],
            EditorKind::Delegate(_) => &["Assignment", "Base branch"],
        };
        for (index, input) in editor.fields.iter().enumerate() {
            let multiline = matches!(editor.kind, EditorKind::Create) && index == 1
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
fn group_header(label: String) -> Div {
    row()
        .w_full()
        .px(px(10.))
        .py(px(5.))
        .line_height(px(18.))
        .rounded(px(7.))
        .text_color(rgb(MUTED))
        .text_size(px(12.))
        .child(label)
}
fn section(label: String, key: &str, closed: bool, cx: &Context<IssuePanel>) -> Stateful<Div> {
    let key = key.to_owned();
    group_header(label)
        .id(SharedString::from(format!("issue-group-{key}")))
        .role(Role::Button)
        .cursor_pointer()
        .child(div().flex_1())
        .child(
            icon(if closed {
                "alt-arrow-right"
            } else {
                "alt-arrow-down"
            })
            .size(px(12.))
            .text_color(rgb(MUTED)),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.collapsed.remove(&key) {
                this.collapsed.insert(key.clone());
            }
            cx.notify();
        }))
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
pub fn status_color(status: ZotIssueStatus) -> u32 {
    match status {
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
                ZotIssueStatus::InProgress | ZotIssueStatus::InReview | ZotIssueStatus::Done
            ),
            |d| d.child(div().size(px(5.)).bg(rgb(color))),
        )
}
fn pr_badge(pr: &ZotIssuePullRequest) -> Div {
    row()
        .flex_none()
        .gap(px(5.))
        .px(px(7.))
        .py(px(2.))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded_full()
        .text_size(px(12.))
        .child(icon("git-branch").size(px(12.)).text_color(rgb(0xd9bf60)))
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
    row()
        .gap(px(6.))
        .pb(px(5.))
        .text_size(px(12.))
        .text_color(rgb(0x959595))
        .child(icon("terminal").size(px(12.)))
        .child(format!("Sent by {}", sender.name))
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
