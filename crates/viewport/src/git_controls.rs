//! Shared native/browser Git panel. The host supplies transport; actions never replay.
use crate::localization::message as tr;
use crate::plugin_setup::SetupColors;
use gpui::{prelude::*, *};
use gpui_base::input::{InputEditorStyle, InputState, TextareaState};
use serde_json::{Value, json};
use zuse_i18n::messages as msg;
use zuse_protocol::{
    GitFileState,
    git_workspace::{GitOperation, GitWorkspace},
    methods,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub chat_id: String,
    pub cwd: String,
    pub device_id: String,
    pub archived: bool,
}
const ROW_HEIGHT: f32 = 26.;
#[derive(Clone, Copy, PartialEq, Eq)]
enum BranchForm {
    Create,
    Switch,
}
#[derive(Clone)]
pub struct Request {
    pub id: u64,
    pub method: &'static str,
    pub params: Value,
}
pub struct GitControls {
    target: Option<Target>,
    snapshot: Option<GitWorkspace>,
    next: u64,
    pending: Option<(u64, bool)>,
    error: Option<String>,
    expanded: bool,
    menu_open: bool,
    branch_form: Option<BranchForm>,
    clear_branch: bool,
    pr_show_repository: bool,
    clear_inputs: bool,
    notice: Option<String>,
    pending_commit: bool,
    message: Entity<InputState>,
    branch: Entity<InputState>,
    pr_title: Entity<InputState>,
    pr_base: Entity<InputState>,
    pr_repository: Entity<InputState>,
    pr_body: Entity<TextareaState>,
    draft_pr: bool,
    creating_pr: bool,
    clear_pr: bool,
    pending_pr: bool,
    submitted_pr: Option<(String, String, String, String, bool)>,
    created_pr: Option<String>,
    scroll: UniformListScrollHandle,
    pub colors: SetupColors,
    applied_colors: SetupColors,
    _inputs: Vec<Subscription>,
    deadline: Option<Task<()>>,
    updated_at: Option<i64>,
    fetch_error: Option<String>,
}
impl EventEmitter<Request> for GitControls {}
impl GitControls {
    pub fn new(colors: SetupColors, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = InputEditorStyle {
            foreground: colors.text.into(),
            muted_foreground: colors.muted.into(),
            background: colors.background.into(),
            border: colors.border.into(),
            caret: colors.text.into(),
            ..Default::default()
        };
        let mut field = |placeholder: &str| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(placeholder.to_owned());
                input.set_editor_style(style.clone());
                input
            })
        };
        let message = field("Commit message");
        let branch = field("Branch name");
        let pr_title = field("Title");
        let pr_base = field("Base branch (optional)");
        let pr_repository = field("owner/repo");
        let pr_body = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx)
                .placeholder(tr(cx, msg::projects::pr_pane_description))
                .auto_grow(2, 5)
                .submit_on_enter(false);
            input.set_editor_style(style.clone());
            input
        });
        let mut subscriptions: Vec<Subscription> =
            [&message, &branch, &pr_title, &pr_base, &pr_repository]
                .into_iter()
                .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
                .collect();
        subscriptions.push(cx.observe(&pr_body, |_, _, cx| cx.notify()));
        Self {
            target: None,
            snapshot: None,
            next: 0,
            pending: None,
            error: None,
            expanded: true,
            menu_open: false,
            branch_form: None,
            clear_branch: false,
            pr_show_repository: false,
            clear_inputs: false,
            notice: None,
            pending_commit: false,
            message,
            branch,
            pr_title,
            pr_base,
            pr_repository,
            pr_body,
            draft_pr: true,
            creating_pr: false,
            clear_pr: false,
            pending_pr: false,
            submitted_pr: None,
            created_pr: None,
            scroll: Default::default(),
            colors,
            applied_colors: colors,
            _inputs: subscriptions,
            deadline: None,
            updated_at: None,
            fetch_error: None,
        }
    }
    pub fn set_target(&mut self, target: Option<Target>, cx: &mut Context<Self>) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.clear_pr = true;
        self.creating_pr = false;
        self.menu_open = false;
        self.branch_form = None;
        self.pr_show_repository = false;
        self.created_pr = None;
        self.snapshot = None;
        self.pending = None;
        self.error = None;
        self.notice = None;
        self.clear_inputs = true;
        self.scroll = Default::default();
        self.deadline = None;
        self.updated_at = None;
        self.fetch_error = None;
        self.watch_current(cx);
        cx.notify();
    }
    pub fn watch_current(&mut self, cx: &mut Context<Self>) {
        let params = self
            .target
            .as_ref()
            .filter(|t| !t.archived)
            .map(|t| json!({"chatId":t.chat_id,"cwd":t.cwd,"targetDeviceId":t.device_id}))
            .unwrap_or_else(|| json!({}));
        cx.emit(Request {
            id: 0,
            method: methods::WATCH_GIT_WORKSPACE,
            params,
        });
        cx.notify();
    }
    pub fn observe(
        &mut self,
        frame: zuse_protocol::git_workspace::SourceControlFrame,
        cx: &mut Context<Self>,
    ) {
        if self.pending.is_some_and(|(_, mutation)| mutation) {
            return;
        }
        if let Some(value) = frame.snapshot.filter(|_| !frame.refreshing)
            && let Ok(snapshot) = serde_json::from_value::<GitWorkspace>(value)
            && self
                .target
                .as_ref()
                .is_some_and(|t| t.chat_id == snapshot.chat_id)
        {
            self.snapshot = Some(snapshot);
            self.updated_at = frame.updated_at;
        }
        self.error = frame.error;
        self.fetch_error = frame.fetch_error;
        cx.notify();
    }
    pub fn observation_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }
    pub fn disconnected(&mut self, cx: &mut Context<Self>) {
        self.deadline = None;
        if self.pending.take().is_some() {
            self.error =
                Some("Connection interrupted. Refresh Git before repeating an action.".into());
        }
        self.snapshot = None;
        cx.notify();
    }
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.watch_current(cx);
        self.send(None, cx);
    }
    fn pr_draft(&self, cx: &App) -> (String, String, String, String, bool) {
        (
            self.pr_title.read(cx).value().to_string(),
            self.pr_body.read(cx).value().to_string(),
            self.pr_base.read(cx).value().to_string(),
            self.pr_repository.read(cx).value().to_string(),
            self.draft_pr,
        )
    }
    fn send(&mut self, operation: Option<GitOperation>, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(target) = self.target.as_ref().filter(|t| !t.archived) else {
            return;
        };
        let mut params =
            json!({"chatId":target.chat_id,"cwd":target.cwd,"targetDeviceId":target.device_id});
        let mutation = operation.is_some();
        self.pending_pr = matches!(
            operation.as_ref(),
            Some(GitOperation::CreatePullRequest { .. })
        );
        if self.pending_pr {
            self.submitted_pr = Some(self.pr_draft(cx));
        }
        self.pending_commit = matches!(operation.as_ref(), Some(GitOperation::Commit { .. }));
        if mutation {
            self.notice = None;
        }
        if let Some(operation) = operation {
            let Some(snapshot) = self.snapshot.as_ref().filter(|s| s.complete) else {
                return;
            };
            params["checkoutId"] = json!(snapshot.checkout_id);
            params["revision"] = json!(snapshot.revision);
            params["operation"] = serde_json::to_value(operation).unwrap();
        }
        self.next = self.next.wrapping_add(1);
        self.pending = Some((self.next, mutation));
        let id = self.next;
        self.deadline = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(if mutation { 150 } else { 30 })).await;
            let _ = this.update(cx, |panel, cx| {
                panel.respond(id, Err(if mutation {
                    "Couldn't confirm the Git action. Check the current status before repeating it.".into()
                } else {
                    "Git status timed out. Check your device connection and refresh.".into()
                }), cx);
            });
        }));
        cx.emit(Request {
            id: self.next,
            method: if mutation {
                methods::GIT_WORKSPACE_ACTION
            } else {
                methods::GET_GIT_WORKSPACE
            },
            params,
        });
        cx.notify();
    }
    pub fn respond(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some((current, mutation)) = self.pending else {
            return;
        };
        if current != id {
            return;
        }
        self.pending = None;
        self.deadline = None;
        match result.and_then(|value| {
            serde_json::from_value::<GitWorkspace>(value)
                .map_err(|_| "Invalid Git response. Refresh before continuing.".into())
        }) {
            Ok(snapshot)
                if self
                    .target
                    .as_ref()
                    .is_some_and(|t| t.chat_id == snapshot.chat_id) =>
            {
                if let Some(url) = snapshot.created_pull_request_url.clone() {
                    self.created_pr = Some(url);
                    self.creating_pr = false;
                    self.clear_pr = self.submitted_pr.as_ref() == Some(&self.pr_draft(cx));
                }
                self.snapshot = Some(snapshot);
                self.error = None;
                if mutation {
                    self.notice = None;
                    if self.pending_commit {
                        self.clear_inputs = true;
                    }
                }
            }
            Ok(_) => {
                self.error = Some("The Git response belongs to another thread.".into());
            }
            Err(error) => {
                if mutation {
                    self.notice = Some(if self.pending_pr {
                        format!("{error} Check pull requests before trying Create again.")
                    } else {
                        error
                    });
                    self.snapshot = None;
                    self.refresh(cx);
                } else {
                    self.error = Some(error);
                }
            }
        }
        cx.notify();
    }
    fn staged(&self) -> Vec<String> {
        self.snapshot
            .as_ref()
            .map(|s| {
                s.files
                    .iter()
                    .filter(|f| {
                        !matches!(f.index, GitFileState::Unchanged | GitFileState::Untracked)
                    })
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn unstaged(&self) -> Vec<String> {
        self.snapshot
            .as_ref()
            .map(|s| {
                s.files
                    .iter()
                    .filter(|f| !matches!(f.worktree, GitFileState::Unchanged))
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn files(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(snapshot) = &self.snapshot else {
            return vec![];
        };
        range
            .filter_map(|i| snapshot.files.get(i).cloned().map(|file| (i, file)))
            .map(|(i, file)| {
                let staged = !matches!(
                    file.index,
                    GitFileState::Unchanged | GitFileState::Untracked
                );
                let work = file.worktree != GitFileState::Unchanged;
                let (dir, name) = match file.path.rsplit_once('/') {
                    Some((dir, name)) => (Some(dir.to_owned()), name.to_owned()),
                    None => (None, file.path.clone()),
                };
                let letter = status_letter(if work { file.worktree } else { file.index });
                let path = file.path;
                let checked = match (staged, work) {
                    (true, false) => Some(true),
                    (false, _) => Some(false),
                    (true, true) => None,
                };
                let hover = self.colors.border.opacity(0.5);
                div()
                    .id(("git-file", i))
                    .role(Role::CheckBox)
                    .aria_label(path.clone())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(ROW_HEIGHT))
                    .px(px(4.))
                    .min_w_0()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(self.checkbox(checked))
                    .child(
                        div()
                            .flex_none()
                            .w(px(10.))
                            .text_size(px(11.))
                            .text_color(self.colors.muted)
                            .child(letter),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.7))
                            .truncate()
                            .text_size(px(12.))
                            .child(name),
                    )
                    .children(dir.map(|dir| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(self.colors.muted)
                            .child(dir)
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let paths = vec![path.clone()];
                        // A partly staged file stages the rest first.
                        this.send(
                            Some(if work {
                                GitOperation::Stage { paths }
                            } else {
                                GitOperation::Unstage { paths }
                            }),
                            cx,
                        )
                    }))
                    .into_any_element()
            })
            .collect()
    }
    /// `Some(true)` checked, `Some(false)` empty, `None` partly staged.
    fn checkbox(&self, checked: Option<bool>) -> Div {
        let colors = self.colors;
        let filled = checked != Some(false);
        div()
            .flex_none()
            .size(px(14.))
            .rounded(px(4.))
            .border_1()
            .border_color(if filled { colors.text } else { colors.muted })
            .when(filled, |d| d.bg(colors.text))
            .flex()
            .items_center()
            .justify_center()
            .when(checked == Some(true), |d| {
                d.child(
                    svg()
                        .path("icons/check.svg")
                        .size(px(10.))
                        .text_color(colors.background),
                )
            })
            .when(checked.is_none(), |d| {
                d.child(
                    div()
                        .w(px(7.))
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(colors.background),
                )
            })
    }
    fn button(&self, id: impl Into<ElementId>, text: &str) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .cursor_pointer()
            .h(px(24.))
            .flex()
            .items_center()
            .px(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(self.colors.border)
            .text_size(px(12.))
            .child(text.to_owned())
            .when(self.pending.is_some(), |d| d.opacity(0.45))
    }
    /// The one filled button a view offers.
    fn primary_button(&self, id: impl Into<ElementId>, text: &str) -> Stateful<Div> {
        self.button(id, text)
            .border_color(self.colors.text)
            .bg(self.colors.text)
            .text_color(self.colors.background)
    }
    fn menu_item(&self, id: &'static str, text: &str) -> Stateful<Div> {
        let hover = self.colors.border.opacity(0.6);
        div()
            .id(id)
            .role(Role::MenuItem)
            .w_full()
            .h(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(px(12.))
            .hover(move |d| d.bg(hover))
            .child(text.to_owned())
    }
    fn menu_separator(&self) -> Div {
        div().my(px(3.)).h(px(1.)).bg(self.colors.border)
    }
    fn run_from_menu(&mut self, operation: GitOperation, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.send(Some(operation), cx);
    }
    fn open_branch_form(&mut self, form: BranchForm, cx: &mut Context<Self>) {
        self.menu_open = false;
        self.creating_pr = false;
        self.branch_form = Some(form);
        self.clear_branch = true;
        cx.notify();
    }
    fn menu(&self, snapshot: &GitWorkspace, cx: &mut Context<Self>) -> impl IntoElement {
        let publish = snapshot.upstream.is_none();
        deferred(
            div()
                .id("git-menu")
                .absolute()
                .top_full()
                .right_0()
                .mt(px(4.))
                .w(px(176.))
                .p(px(4.))
                .rounded(px(10.))
                .border_1()
                .border_color(self.colors.border)
                .bg(crate::palette::rgb(crate::shell::RAISED))
                .text_color(self.colors.text)
                .shadow_lg()
                .occlude()
                .role(Role::Menu)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.menu_open = false;
                    cx.notify();
                }))
                .child(self.menu_item("git-fetch", "Fetch").on_click(
                    cx.listener(|this, _, _, cx| this.run_from_menu(GitOperation::Fetch, cx)),
                ))
                .when(!publish, |d| {
                    d.child(self.menu_item("git-pull", "Pull").on_click(
                        cx.listener(|this, _, _, cx| this.run_from_menu(GitOperation::Pull, cx)),
                    ))
                })
                .child(
                    self.menu_item("git-push", if publish { "Publish branch" } else { "Push" })
                        .on_click(
                            cx.listener(|this, _, _, cx| {
                                this.run_from_menu(GitOperation::Push, cx)
                            }),
                        ),
                )
                .child(self.menu_separator())
                .child(
                    self.menu_item("git-create-pr", "Create pull request…")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu_open = false;
                            this.branch_form = None;
                            this.creating_pr = true;
                            cx.notify();
                        })),
                )
                .child(self.menu_item("git-new-branch", "New branch…").on_click(
                    cx.listener(|this, _, _, cx| this.open_branch_form(BranchForm::Create, cx)),
                ))
                .child(
                    self.menu_item("git-switch-branch", "Switch branch…")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_branch_form(BranchForm::Switch, cx)
                        })),
                )
                .child(self.menu_separator())
                .child(
                    self.menu_item("git-refresh", "Refresh")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu_open = false;
                            this.refresh(cx);
                        })),
                ),
        )
    }
}
impl Render for GitControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.applied_colors != self.colors {
            self.applied_colors = self.colors;
            let c = self.colors;
            let style = InputEditorStyle {
                foreground: c.text.into(),
                muted_foreground: c.muted.into(),
                background: c.background.into(),
                border: c.border.into(),
                caret: c.text.into(),
                ..Default::default()
            };
            for field in [
                &self.message,
                &self.branch,
                &self.pr_title,
                &self.pr_base,
                &self.pr_repository,
            ] {
                field.update(cx, |input, _| input.set_editor_style(style.clone()));
            }
            self.pr_body
                .update(cx, |input, _| input.set_editor_style(style));
        }
        if self.clear_pr {
            self.clear_pr = false;
            for field in [&self.pr_title, &self.pr_base, &self.pr_repository] {
                field.update(cx, |input, cx| input.set_value("", window, cx));
            }
            self.pr_body
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if self.clear_inputs {
            self.clear_inputs = false;
            self.clear_branch = true;
            self.message
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        if self.clear_branch {
            self.clear_branch = false;
            self.branch
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        let colors = self.colors;
        let target = self.target.as_ref();
        let archived = target.is_some_and(|t| t.archived);
        let available = target.is_some() && !archived;
        let branch = self
            .snapshot
            .as_ref()
            .map(|s| s.branch.clone())
            .unwrap_or_else(|| "Git".into());
        let staged = self.staged();
        let unstaged = self.unstaged();
        let failure = self
            .notice
            .clone()
            .or(self.error.clone())
            .or(self.fetch_error.clone());
        let line =
            |text: String, color: Hsla| div().text_size(px(11.)).text_color(color).child(text);
        let counter = |glyph: &str, count: u32| {
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(colors.muted)
                .child(format!("{glyph}{count}"))
        };
        let (ahead, behind) = self
            .snapshot
            .as_ref()
            .map_or((0, 0), |s| (s.ahead, s.behind));
        let primary = self
            .snapshot
            .as_ref()
            .filter(|_| available && self.expanded)
            .and_then(|s| primary_action(s, self.created_pr.as_deref()));
        let primary = primary.map(|action| match action {
            PrimaryAction::Publish => self
                .primary_button("git-primary", "Publish")
                .on_click(cx.listener(|this, _, _, cx| this.send(Some(GitOperation::Push), cx))),
            PrimaryAction::Pull => self
                .primary_button("git-primary", &format!("Pull {behind}"))
                .on_click(cx.listener(|this, _, _, cx| this.send(Some(GitOperation::Pull), cx))),
            PrimaryAction::Push => self
                .primary_button("git-primary", &format!("Push {ahead}"))
                .on_click(cx.listener(|this, _, _, cx| this.send(Some(GitOperation::Push), cx))),
            PrimaryAction::OpenPullRequest(url) => self
                .primary_button("git-primary", "Open PR")
                .on_click(move |_, _, cx| cx.open_url(&url)),
        });
        let menu = self
            .snapshot
            .as_ref()
            .filter(|_| available && self.expanded)
            .map(|snapshot| {
                let hover = colors.border.opacity(0.6);
                div()
                    .relative()
                    .flex_none()
                    .child(
                        div()
                            .id("git-more")
                            .role(Role::Button)
                            .aria_label("More Git actions")
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(move |d| d.bg(hover))
                            .child(
                                svg()
                                    .path("icons/more-horizontal.svg")
                                    .size(px(14.))
                                    .text_color(colors.muted),
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.menu_open = !this.menu_open;
                                cx.notify();
                            })),
                    )
                    .when(self.menu_open, |d| d.child(self.menu(snapshot, cx)))
            });
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .child(
                div()
                    .id("git-panel-toggle")
                    .role(Role::Button)
                    .aria_label(format!("Git, branch {branch}"))
                    .aria_expanded(self.expanded)
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(if self.expanded {
                                "icons/alt-arrow-down.svg"
                            } else {
                                "icons/alt-arrow-right.svg"
                            })
                            .flex_none()
                            .size(px(11.))
                            .text_color(colors.muted),
                    )
                    .child(
                        svg()
                            .path("icons/git-branch.svg")
                            .flex_none()
                            .size(px(13.))
                            .text_color(colors.muted),
                    )
                    .child(div().min_w_0().truncate().text_size(px(12.)).child(branch))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.expanded = !this.expanded;
                        this.menu_open = false;
                        if this.expanded && this.snapshot.is_none() {
                            this.refresh(cx);
                        }
                        cx.notify();
                    })),
            )
            .when(ahead > 0, |d| d.child(counter("↑", ahead)))
            .when(behind > 0, |d| d.child(counter("↓", behind)))
            .child(div().flex_1())
            .children(primary)
            .children(menu);
        let mut view = div()
            .flex_none()
            .flex()
            .flex_col()
            .text_color(colors.text)
            .border_b_1()
            .border_color(colors.border)
            .px(px(10.))
            .py(px(4.))
            .gap(px(6.))
            .child(header);
        if !self.expanded {
            return view.when_some(self.error.clone(), |view, error| {
                view.child(line(error, colors.danger))
            });
        }
        if !available {
            return view.child(line(
                if archived {
                    "Unarchive this thread to use Git."
                } else {
                    "No Git checkout for this thread."
                }
                .to_owned(),
                colors.muted,
            ));
        }
        if let Some(snapshot) = &self.snapshot {
            if self.creating_pr {
                let draft = self.draft_pr;
                let show_repository = self.pr_show_repository;
                let create = self
                    .primary_button(
                        "git-create-pr-submit",
                        if self.pending.is_some() {
                            "Creating…"
                        } else {
                            "Create"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let title = this.pr_title.read(cx).value().to_string();
                        let body = this.pr_body.read(cx).value().to_string();
                        let base = this.pr_base.read(cx).value().trim().to_owned();
                        let repository = this.pr_repository.read(cx).value().trim().to_owned();
                        this.send(
                            Some(GitOperation::CreatePullRequest {
                                title,
                                body,
                                base,
                                base_repository: (!repository.is_empty()).then_some(repository),
                                draft: this.draft_pr,
                            }),
                            cx,
                        );
                    }));
                return view
                    .child(self.pr_title.clone())
                    .child(self.pr_body.clone())
                    .child(self.pr_base.clone())
                    .when(show_repository, |d| d.child(self.pr_repository.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                crate::settings_style::toggle(
                                    "git-create-pr-draft",
                                    draft,
                                    "Draft",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if this.pending.is_none() {
                                            this.draft_pr = !this.draft_pr;
                                            cx.notify();
                                        }
                                    },
                                )),
                            )
                            .when(!show_repository, |d| {
                                d.child(
                                    div()
                                        .id("git-create-pr-repository")
                                        .cursor_pointer()
                                        .text_size(px(11.))
                                        .text_color(colors.muted)
                                        .child("Other repository…")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.pr_show_repository = true;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .child(self.button("git-create-pr-cancel", "Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    if this.pending.is_none() {
                                        this.creating_pr = false;
                                        cx.notify();
                                    }
                                }),
                            ))
                            .child(create),
                    )
                    .when_some(failure, |d, error| d.child(line(error, colors.danger)));
            }

            if let Some(form) = self.branch_form {
                let (id, label) = match form {
                    BranchForm::Create => ("git-branch-create", "Create"),
                    BranchForm::Switch => ("git-branch-switch", "Switch"),
                };
                view = view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().flex_1().min_w_0().child(self.branch.clone()))
                        .child(
                            self.button("git-branch-cancel", "Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.branch_form = None;
                                    cx.notify();
                                })),
                        )
                        .child(self.primary_button(id, label).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let name = this.branch.read(cx).value().trim().to_owned();
                                if name.is_empty() {
                                    return;
                                }
                                this.branch_form = None;
                                this.send(
                                    Some(match form {
                                        BranchForm::Create => GitOperation::CreateBranch { name },
                                        BranchForm::Switch => GitOperation::SwitchBranch { name },
                                    }),
                                    cx,
                                );
                            },
                        ))),
                );
            }
            view = view.when(!snapshot.complete, |d| {
                d.child(line(
                    tr(cx, msg::native::partial_status_actions_are_unavailable).to_string(),
                    colors.danger,
                ))
            });
            if snapshot.files.is_empty() {
                view = view.child(line("No local changes".to_owned(), colors.muted));
            } else {
                let total = snapshot.files.len();
                let checked = if unstaged.is_empty() {
                    Some(true)
                } else if staged.is_empty() {
                    Some(false)
                } else {
                    None
                };
                let staged_count = staged.len();
                let message_empty = self.message.read(cx).value().trim().is_empty();
                let can_commit = staged_count > 0 && !message_empty;
                view = view
                    .child(
                        div()
                            .id("git-stage-all")
                            .role(Role::CheckBox)
                            .aria_label("Stage all changes")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(ROW_HEIGHT))
                            .px(px(4.))
                            .cursor_pointer()
                            .child(self.checkbox(checked))
                            .child(div().text_size(px(12.)).child(format!(
                                "{total} {}",
                                if total == 1 { "change" } else { "changes" }
                            )))
                            .child(div().flex_1())
                            .when(staged_count > 0, |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(colors.muted)
                                        .child(format!("{staged_count} staged")),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(
                                    Some(if unstaged.is_empty() {
                                        GitOperation::Unstage {
                                            paths: staged.clone(),
                                        }
                                    } else {
                                        GitOperation::Stage {
                                            paths: unstaged.clone(),
                                        }
                                    }),
                                    cx,
                                );
                            })),
                    )
                    .child(
                        uniform_list("git-changed-files", total, cx.processor(Self::files))
                            .h(px(total.min(6) as f32 * ROW_HEIGHT))
                            .track_scroll(&self.scroll),
                    )
                    .child(
                        div()
                            .pb(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().min_w_0().child(self.message.clone()))
                            .child(if can_commit {
                                self.primary_button("git-commit", "Commit")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let message = this.message.read(cx).value().to_string();
                                        this.send(Some(GitOperation::Commit { message }), cx);
                                    }))
                            } else {
                                self.button("git-commit", "Commit").opacity(0.45)
                            }),
                    );
            }
        } else if self.error.is_none() {
            view = view.child(line("Loading…".to_owned(), colors.muted));
        }
        view.when_some(failure, |d, error| d.child(line(error, colors.danger)))
    }
}

/// The next thing this branch needs, shown as the panel's one filled button.
/// Everything else stays in the overflow menu.
#[derive(Debug, PartialEq, Eq)]
enum PrimaryAction {
    Publish,
    Pull,
    Push,
    OpenPullRequest(String),
}

fn primary_action(snapshot: &GitWorkspace, created: Option<&str>) -> Option<PrimaryAction> {
    if snapshot.upstream.is_none() {
        return snapshot.head_sha.as_ref().map(|_| PrimaryAction::Publish);
    }
    // Pull comes first: a diverged branch cannot push until it integrates.
    if snapshot.behind > 0 {
        Some(PrimaryAction::Pull)
    } else if snapshot.ahead > 0 {
        Some(PrimaryAction::Push)
    } else {
        created.map(|url| PrimaryAction::OpenPullRequest(url.to_owned()))
    }
}

fn status_letter(state: GitFileState) -> &'static str {
    match state {
        GitFileState::Added => "A",
        GitFileState::Modified => "M",
        GitFileState::Deleted => "D",
        GitFileState::Renamed => "R",
        GitFileState::Copied => "C",
        GitFileState::Unmerged => "!",
        GitFileState::Untracked => "U",
        GitFileState::TypeChanged => "T",
        GitFileState::Unchanged => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, prelude::v1::test, rc::Rc};
    fn snapshot(chat: &str) -> Value {
        json!({"chatId":chat,"checkoutId":chat,"cwd":"/repo","branch":"main","headSha":"head","upstream":null,"ahead":0,"behind":0,"files":[{"path":"file","oldPath":null,"index":"modified","worktree":"unchanged"}],"complete":true,"revision":"revision"})
    }

    #[test]
    fn the_filled_button_is_the_branch_next_step() {
        let base = |upstream: Option<&str>, ahead, behind| GitWorkspace {
            upstream: upstream.map(str::to_owned),
            ahead,
            behind,
            ..serde_json::from_value(snapshot("chat")).unwrap()
        };
        let origin = Some("origin/feature");
        assert_eq!(
            primary_action(&base(None, 0, 0), None),
            Some(PrimaryAction::Publish)
        );
        assert_eq!(
            primary_action(&base(origin, 2, 1), None),
            Some(PrimaryAction::Pull),
            "a diverged branch integrates before it can push"
        );
        assert_eq!(
            primary_action(&base(origin, 2, 0), None),
            Some(PrimaryAction::Push)
        );
        assert_eq!(primary_action(&base(origin, 0, 0), None), None);
        assert_eq!(
            primary_action(&base(origin, 0, 0), Some("https://example.test/1")),
            Some(PrimaryAction::OpenPullRequest(
                "https://example.test/1".into()
            ))
        );
    }

    #[gpui::test]
    fn initial_status_times_out_and_a_late_receipt_cannot_replace_the_retry(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::initialize);
        let window = cx.add_window(|window, cx| {
            let color = gpui::rgb(0x202020).into();
            GitControls::new(
                SetupColors {
                    text: color,
                    muted: color,
                    background: color,
                    border: color,
                    danger: color,
                },
                window,
                cx,
            )
        });
        let old = window
            .update(cx, |panel, _, cx| {
                panel.set_target(
                    Some(Target {
                        chat_id: "chat".into(),
                        cwd: "/repo".into(),
                        device_id: "host".into(),
                        archived: false,
                    }),
                    cx,
                );
                panel.refresh(cx);
                panel
                    .pending
                    .expect("explicit refresh starts a bounded status read")
                    .0
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(31));
        cx.run_until_parked();
        window
            .update(cx, |panel, _, cx| {
                assert!(panel.pending.is_none());
                assert!(panel.error.as_ref().unwrap().contains("timed out"));
                panel.refresh(cx);
                let retry = panel.pending.unwrap().0;
                panel.respond(old, Ok(snapshot("chat")), cx);
                assert!(
                    panel.snapshot.is_none(),
                    "the timed-out read cannot win over the retry"
                );
                panel.respond(retry, Ok(snapshot("chat")), cx);
                assert!(panel.snapshot.is_some() && panel.error.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn panel_fences_old_thread_receipts_and_refreshes_without_replaying_mutations(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::initialize);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let captured = calls.clone();
        let window = cx.add_window(|window, cx| {
            let color: Hsla = gpui::rgb(0x202020).into();
            GitControls::new(
                SetupColors {
                    text: color,
                    muted: color,
                    background: color,
                    border: color,
                    danger: color,
                },
                window,
                cx,
            )
        });
        let panel = window.root(cx).unwrap();
        let _events = cx.update(|cx| {
            cx.subscribe(&panel, move |_, event: &Request, _| {
                captured.borrow_mut().push(event.clone())
            })
        });
        window
            .update(cx, |view, window, cx| {
                view.set_target(
                    Some(Target {
                        chat_id: "a".into(),
                        cwd: "/repo".into(),
                        device_id: "host".into(),
                        archived: false,
                    }),
                    cx,
                );
                view.refresh(cx);
                let id = view.pending.unwrap().0;
                view.respond(id, Ok(snapshot("a")), cx);
                view.send(
                    Some(GitOperation::Stage {
                        paths: vec!["file".into()],
                    }),
                    cx,
                );
                let stale = view.pending.unwrap().0;
                view.set_target(
                    Some(Target {
                        chat_id: "b".into(),
                        cwd: "/repo".into(),
                        device_id: "host".into(),
                        archived: false,
                    }),
                    cx,
                );
                view.respond(stale, Ok(snapshot("a")), cx);
                assert!(view.snapshot.is_none());
                view.refresh(cx);
                let id = view.pending.unwrap().0;
                view.respond(id, Ok(snapshot("b")), cx);
                view.send(
                    Some(GitOperation::Commit {
                        message: "Commit".into(),
                    }),
                    cx,
                );
                let id = view.pending.unwrap().0;
                view.respond(id, Err("Confirmation lost".into()), cx);
                assert_eq!(view.pending.unwrap().1, false);
                let read = view.pending.unwrap().0;
                view.respond(read, Ok(snapshot("b")), cx);
                assert_eq!(view.notice.as_deref(), Some("Confirmation lost"));
                view.pr_title.update(cx, |input, cx| {
                    input.set_value("Original title", window, cx)
                });
                let create = GitOperation::CreatePullRequest {
                    title: "Original title".into(),
                    body: "".into(),
                    base: "".into(),
                    base_repository: None,
                    draft: true,
                };
                view.send(Some(create.clone()), cx);
                let id = view.pending.unwrap().0;
                view.respond(id, Err("Receipt lost".into()), cx);
                assert!(
                    view.notice
                        .as_ref()
                        .unwrap()
                        .contains("Check pull requests")
                );
                let read = view.pending.unwrap().0;
                view.respond(read, Ok(snapshot("b")), cx);
                assert_eq!(view.pr_title.read(cx).value().as_ref(), "Original title");
                view.send(Some(create), cx);
                let id = view.pending.unwrap().0;
                view.pr_title
                    .update(cx, |input, cx| input.set_value("New draft", window, cx));
                let mut receipt = snapshot("b");
                receipt["createdPullRequestUrl"] = json!("https://github.com/acme/app/pull/1");
                view.clear_pr = false;
                view.respond(id, Ok(receipt), cx);
                assert!(
                    !view.clear_pr,
                    "a successful creation cannot clear a newer draft"
                );
                assert_eq!(
                    view.created_pr.as_deref(),
                    Some("https://github.com/acme/app/pull/1")
                );
                view.set_target(
                    Some(Target {
                        chat_id: "b".into(),
                        cwd: "/repo".into(),
                        device_id: "host".into(),
                        archived: true,
                    }),
                    cx,
                );
                view.refresh(cx);
                assert!(view.pending.is_none());
            })
            .unwrap();
        let calls = calls.borrow();
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.method == methods::GIT_WORKSPACE_ACTION)
                .count(),
            4
        );
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.method == methods::GET_GIT_WORKSPACE)
                .count(),
            4
        );
    }
}
