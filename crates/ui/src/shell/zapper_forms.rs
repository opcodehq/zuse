//! Zot forms. New Zot and Zot settings are routed pages; documents, decisions
//! and threads float over the conversation they came from.
use super::zot_avatar::ZapperFace;
use super::zots::{zot_avatar, zot_mascot};
use super::*;
use crate::settings::plugins::catalog_ui;
use crate::settings::widgets::{self, ActionTone};
use zuse_protocol::presentation::zot_mascots;
use zuse_protocol::zots::*;

pub(super) enum Kind {
    Config,
    File(ZotFile),
    Thread(Option<String>),
    Decision(ZotDecision),
}

pub(super) struct Form {
    pub(super) id: String,
    zot: Option<Zot>,
    kind: Kind,
    inputs: Vec<Entity<ComposerInput>>,
    repositories: Vec<ZotRepository>,
    /// The project a new thread starts in, when the Zot has several.
    space: Option<String>,
    home: String,
    worker_device: String,
    avatar: ZotAvatar,
    /// The user's own picture as a `data:` URL; shown instead of `avatar`.
    pub(super) image: Option<String>,
    /// Projects that existed when "Add" was pressed; a new one is switched on.
    known_spaces: Option<Vec<String>>,
    /// Filters the repository list on New Zapper and the Zapper editor.
    repo_search: Option<Entity<ComposerInput>>,
    /// Keyboard highlight in the filtered repository list.
    repo_active: Option<usize>,
    repo_scroll: gpui::ScrollHandle,
    page_scroll: gpui::ScrollHandle,
    advanced_open: bool,
    max_workers: u32,
    max_followups: u32,
    focus_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl Form {
    fn target(&self) -> Option<String> {
        self.zot.as_ref().map(|z| z.id.clone())
    }
}

/// What a successful command does to the surface that sent it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ZotAfter {
    Stay,
    CloseDialog,
    /// The page's history entry becomes the new coordinator, so Back skips
    /// the finished form.
    Created,
    LeavePage,
    OpenCoordinator,
}

#[derive(Clone, Copy)]
enum Lines {
    Single,
    /// Long text where Enter starts a new line.
    Prose,
    /// A short message where Enter sends.
    Message,
}

fn fields(kind: &Kind, zot: Option<&Zot>) -> Vec<(&'static str, String, Lines)> {
    match kind {
        Kind::Config => vec![
            (
                "Name",
                zot.map(|z| z.config.name.clone()).unwrap_or_default(),
                Lines::Single,
            ),
            (
                PURPOSE_HINT,
                zot.map(|z| z.config.purpose.clone()).unwrap_or_default(),
                Lines::Prose,
            ),
        ],
        Kind::File(file) if file.version == 0 => vec![
            ("Name", String::new(), Lines::Single),
            ("Content", String::new(), Lines::Prose),
        ],
        Kind::File(file) => vec![("Content", file.content.clone(), Lines::Prose)],
        Kind::Decision(_) => vec![("Your answer", String::new(), Lines::Message)],
        Kind::Thread(Some(_)) => vec![("Message", String::new(), Lines::Message)],
        Kind::Thread(None) => vec![
            ("What should this thread do?", String::new(), Lines::Message),
            ("HEAD", String::new(), Lines::Single),
        ],
    }
}

/// A text box whose whole frame focuses its input, not just the first line.
fn text_box(input: Entity<ComposerInput>, min_height: f32, cx: &App) -> gpui::Div {
    let focus = input.focus_handle(cx);
    popover::dialog_field(input.into_any_element())
        .min_h(px(min_height))
        .cursor_text()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let focus = focus.clone();
            // The input blurs on presses outside its own line box.
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        })
}

/// Space between a field's label and its control, everywhere on the form.
const LABEL_GAP: f32 = 8.0;
const PURPOSE_HINT: &str = "Leave blank and it will ask you after it starts.";
/// Compact one-line repository rows; the list shows about seven before it
/// scrolls, the last one cut in half as a hint.
const REPO_ROW_HEIGHT: f32 = 30.0;
const REPO_LIST_HEIGHT: f32 = 4.0 + 7.5 * (REPO_ROW_HEIGHT + 1.0);
const MAX_WORKERS: std::ops::RangeInclusive<u32> = 1..=32;
const MAX_FOLLOWUPS: std::ops::RangeInclusive<u32> = 0..=32;

/// The muted line under a field label.
fn hint(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(11.5))
        .text_color(theme.text_faint)
        .child(text.into())
}

/// A field label with an optional hint under it and a control on the right.
fn field_heading(
    theme: &Theme,
    label: &'static str,
    detail: Option<&'static str>,
    trailing: Option<AnyElement>,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(widgets::field_label(theme, label))
                .children(detail.map(|d| hint(theme, d))),
        )
        .children(trailing)
}

/// One project as the repository picker lists it.
#[derive(Clone, Debug, PartialEq)]
struct RepoChoice {
    space_id: String,
    device_id: String,
    /// `owner/repo` when the project has a hosted remote, else its name.
    name: String,
    path: String,
    /// Whether `name` came from the remote rather than the folder.
    hosted: bool,
    /// Set when the projects span more than one device.
    device: Option<String>,
}

/// `owner/repo` from a stamped remote (`host/owner/repo`).
fn remote_name(remote: &str) -> Option<String> {
    let (_host, path) = remote.trim_end_matches(".git").split_once('/')?;
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let owner = segments.next()?;
    let repo = segments.next()?;
    Some(format!("{owner}/{repo}"))
}

fn space_name(space: &zuse_protocol::Space) -> (String, bool) {
    match space.remote_repository.as_deref().and_then(remote_name) {
        Some(name) => (name, true),
        None => (space.display_name().to_owned(), false),
    }
}

fn repo_choices(
    spaces: &[zuse_protocol::Space],
    devices: &[zuse_protocol::Device],
) -> Vec<RepoChoice> {
    let several = spaces.iter().any(|s| {
        spaces
            .first()
            .is_some_and(|first| first.device_id != s.device_id)
    });
    spaces
        .iter()
        .map(|space| {
            let (name, hosted) = space_name(space);
            RepoChoice {
                space_id: space.id.clone(),
                device_id: space.device_id.clone(),
                name,
                path: space.path.clone(),
                hosted,
                device: several.then(|| {
                    devices
                        .iter()
                        .find(|d| d.id == space.device_id)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| space.device_id.clone())
                }),
            }
        })
        .collect()
}

/// Indices of the choices whose name or path contains the query, ignoring
/// case. Name and path prefixes rank first; order is otherwise kept.
fn filter_repo_choices(query: &str, choices: &[RepoChoice]) -> Vec<usize> {
    let mut ranked: Vec<(usize, usize)> = choices
        .iter()
        .enumerate()
        .filter_map(|(index, choice)| {
            let name = popover::match_rank(query, &choice.name);
            let path = popover::match_rank(query, &choice.path);
            name.into_iter().chain(path).min().map(|rank| (rank, index))
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, index)| index).collect()
}

/// Switch a repository on or off. Returns whether it is now selected.
fn toggle_repository(selected: &mut Vec<ZotRepository>, choice: &RepoChoice) -> bool {
    if selected.iter().any(|r| r.space_id == choice.space_id) {
        selected.retain(|r| r.space_id != choice.space_id);
        false
    } else {
        selected.push(ZotRepository {
            space_id: choice.space_id.clone(),
            device_id: choice.device_id.clone(),
        });
        true
    }
}

fn step_limit(value: u32, delta: i32, range: &std::ops::RangeInclusive<u32>) -> u32 {
    value
        .saturating_add_signed(delta)
        .clamp(*range.start(), *range.end())
}

fn labelled(
    theme: &Theme,
    label: &'static str,
    input: Entity<ComposerInput>,
    min_height: f32,
    cx: &App,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(LABEL_GAP))
        .child(widgets::field_label(theme, label))
        .child(text_box(input, min_height, cx))
}

fn chip(
    id: impl Into<SharedString>,
    label: String,
    active: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .role(gpui::Role::RadioButton)
        .aria_label(SharedString::from(label.clone()))
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .px(px(10.0))
        .rounded_full()
        .border_1()
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(12.0))
        .when(active, |d| {
            d.border_color(theme.border_strong)
                .bg(theme.element_active)
                .text_color(theme.text_strong)
                .child(
                    icon(icons::CHECK)
                        .size(px(11.0))
                        .flex_none()
                        .text_color(theme.text_strong),
                )
        })
        .when(!active, |d| {
            d.border_color(theme.border)
                .text_color(theme.text_muted)
                .hover(|d| d.text_color(theme.text))
        })
        .child(label)
}

impl Shell {
    /// On the New Zot page.
    pub(super) fn creating_zot(&self) -> bool {
        matches!(self.route, Route::Zot)
    }

    /// Whether the right panel is editing the selected Zot.
    pub(super) fn zot_editor_open(&self, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && self.selected_zot(cx).is_some_and(|zot| {
                self.zot_ui
                    .page
                    .as_ref()
                    .and_then(|page| page.target())
                    .is_some_and(|target| target == zot.id)
            })
    }

    pub(super) fn open_zot_editor(&mut self, zot: Zot, cx: &mut Context<Self>) {
        self.zot_ui.page = Some(self.build_zot_form(Some(zot), Kind::Config, false, cx));
        cx.notify();
    }

    fn build_zot_form(
        &mut self,
        zot: Option<Zot>,
        kind: Kind,
        dialog: bool,
        cx: &mut Context<Self>,
    ) -> Form {
        let state = self.state.read(cx);
        let home = zot
            .as_ref()
            .map(|z| z.config.home_device_id.clone())
            .or_else(|| state.local_device_id.clone())
            .unwrap_or_default();
        let repositories = zot
            .as_ref()
            .map(|z| z.config.repositories.clone())
            .unwrap_or_else(|| {
                state
                    .selected_space_row()
                    .map(|s| {
                        vec![ZotRepository {
                            device_id: s.device_id.clone(),
                            space_id: s.id.clone(),
                        }]
                    })
                    .unwrap_or_default()
            });
        let worker_device = zot
            .as_ref()
            .map(|z| z.config.worker_device_id.clone())
            .unwrap_or_else(|| home.clone());
        let id = zot
            .as_ref()
            .map(|z| z.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let face = zot
            .as_ref()
            .map(zot_avatar)
            .unwrap_or_else(|| ZapperFace::mascot(zot_mascots::default_avatar(&id)));
        let (avatar, image) = (face.avatar, face.image.map(String::from));
        let mut subscriptions = vec![];
        let inputs = fields(&kind, zot.as_ref())
            .into_iter()
            .map(|(placeholder, value, lines)| {
                let input = cx.new(|cx| {
                    match lines {
                        Lines::Single => {
                            ComposerInput::with_context(placeholder, "ProjectForm", cx)
                                .with_single_line()
                        }
                        Lines::Prose => ComposerInput::new(placeholder, cx).with_enter_newline(),
                        Lines::Message => ComposerInput::new(placeholder, cx),
                    }
                    .with_tab_stop()
                });
                input.update(cx, |input, cx| input.set_text(value, cx));
                subscriptions.push(cx.subscribe(
                    &input,
                    move |this: &mut Shell, _, event: &ComposerInputEvent, cx| match event {
                        ComposerInputEvent::Submitted => this.submit_zot_form(dialog, cx),
                        ComposerInputEvent::Edited => cx.notify(),
                        _ => {}
                    },
                ));
                input
            })
            .collect();
        let repo_search = matches!(kind, Kind::Config).then(|| {
            let input = cx.new(|cx| {
                ComposerInput::with_context("Search by name or path", "PaletteSearch", cx)
                    .with_text_metrics(crate::typography::TEXT_XS, 16.0)
                    .with_accessibility_role(gpui::Role::SearchInput)
                    .with_single_line()
                    .with_tab_stop()
            });
            subscriptions.push(cx.subscribe(
                &input,
                |this: &mut Shell, input, event: &ComposerInputEvent, cx| {
                    if matches!(event, ComposerInputEvent::Edited) {
                        let searching = !input.read(cx).text().trim().is_empty();
                        if let Some(form) = this.zot_ui.page.as_mut() {
                            // Typing highlights the best match, so Enter picks it.
                            form.repo_active = searching.then_some(0);
                            form.repo_scroll.scroll_to_item(0);
                        }
                        cx.notify();
                    }
                },
            ));
            input
        });
        let (max_workers, max_followups) = zot
            .as_ref()
            .map(|z| (z.config.max_concurrent_workers, z.config.max_followups))
            .unwrap_or((4, 3));
        Form {
            id,
            zot,
            kind,
            inputs,
            repositories,
            space: None,
            home,
            worker_device,
            avatar,
            image,
            known_spaces: None,
            repo_search,
            repo_active: None,
            repo_scroll: gpui::ScrollHandle::new(),
            page_scroll: gpui::ScrollHandle::new(),
            advanced_open: false,
            max_workers,
            max_followups,
            focus_pending: true,
            _subscriptions: subscriptions,
        }
    }

    /// Visual fixture hook: open New Zapper.
    pub fn fixture_open_new_zapper(&mut self, cx: &mut Context<Self>) {
        self.open_new_zot(cx);
    }

    /// Visual fixture hook: type into the repository search.
    pub fn fixture_search_repositories(&mut self, query: &str, cx: &mut Context<Self>) -> bool {
        let Some(input) = self
            .zot_ui
            .page
            .as_ref()
            .and_then(|f| f.repo_search.clone())
        else {
            return false;
        };
        input.update(cx, |input, cx| input.set_text(query, cx));
        true
    }

    /// Visual fixture hook: scroll New Zapper to its end.
    pub fn fixture_scroll_zapper_page(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = self.zot_ui.page.as_ref() {
            form.page_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Visual fixture hook: expand Advanced.
    pub fn fixture_open_advanced(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(form) = self.zot_ui.page.as_mut() else {
            return false;
        };
        form.advanced_open = true;
        cx.notify();
        true
    }

    pub(super) fn open_new_zot(&mut self, cx: &mut Context<Self>) {
        self.nav.push(NavEntry::NewZot);
        self.show_new_zot(cx);
    }

    /// Land on New Zot, keeping a draft the user walked away from.
    pub(super) fn show_new_zot(&mut self, cx: &mut Context<Self>) {
        match self.zot_ui.page.as_mut() {
            Some(page) if page.target().is_none() => page.focus_pending = true,
            _ => self.zot_ui.page = Some(self.build_zot_form(None, Kind::Config, false, cx)),
        }
        self.zot_ui.dialog = None;
        self.set_voice_stage_open(false, cx);
        self.set_route(Route::Zot, cx);
        cx.notify();
    }

    /// Discard the page and return to wherever it was opened from.
    pub(super) fn leave_zot_page(&mut self, cx: &mut Context<Self>) {
        self.zot_ui.page = None;
        if !matches!(self.route, Route::Zot) {
            return;
        }
        if self.nav.can_back() {
            self.navigate_back(cx);
        } else {
            self.apply_nav(NavEntry::Chat(String::new()), cx);
        }
    }

    pub(super) fn open_zot_dialog(&mut self, zot: Zot, kind: Kind, cx: &mut Context<Self>) {
        self.zot_ui.dialog = Some(self.build_zot_form(Some(zot), kind, true, cx));
        cx.notify();
    }

    fn zot_default_model(&self, cx: &App) -> ZotModel {
        let chosen = crate::settings::default_model(cx).map(|preferred| ZotModel {
            harness: String::from(preferred.provider.clone()),
            model: Some(preferred.model.clone()),
        });
        chosen
            .or_else(|| {
                let config = self.state.read(cx).selected_chat_row()?.config.clone()?;
                Some(ZotModel {
                    harness: String::from(config.harness),
                    model: config.model,
                })
            })
            .unwrap_or_else(|| ZotModel {
                harness: "claude-code".into(),
                model: None,
            })
    }

    pub(super) fn zot_thread_model(&self, zot: &Zot, cx: &App) -> ZotModel {
        self.state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == zot.coordinator_chat_id)
            .and_then(|c| c.config.clone())
            .map(|config| ZotModel {
                harness: String::from(config.harness),
                model: config.model,
            })
            .unwrap_or_else(|| zot.config.coordinator_model.clone())
    }

    pub(super) fn submit_zot_form(&mut self, dialog: bool, cx: &mut Context<Self>) {
        let form = if dialog {
            self.zot_ui.dialog.as_ref()
        } else {
            self.zot_ui.page.as_ref()
        };
        let Some(form) = form else {
            return;
        };
        let values: Vec<String> = form
            .inputs
            .iter()
            .map(|i| i.read(cx).text().to_owned())
            .collect();
        let error = |this: &mut Self, text: &'static str, cx: &mut Context<Self>| {
            this.show_notice(crate::toast::ToastKind::Error, text, cx)
        };
        // Revisions also move for decisions and worker activity, so a form that
        // has been open a while still writes against the current one.
        let latest = form.zot.as_ref().and_then(|zot| {
            self.state
                .read(cx)
                .zots
                .iter()
                .find(|z| z.id == zot.id)
                .cloned()
        });
        if form.zot.is_some() && latest.is_none() {
            error(self, "This Zapper no longer exists", cx);
            return;
        }
        let revision = latest.as_ref().map(|z| z.revision);
        let mut after = if dialog {
            ZotAfter::CloseDialog
        } else {
            ZotAfter::LeavePage
        };
        let action = match &form.kind {
            Kind::Config => {
                let name = values[0].trim().to_owned();
                if name.is_empty() {
                    error(self, "Give this Zapper a name", cx);
                    return;
                }
                let (home_device_id, repositories, worker_device_id) = (
                    form.home.clone(),
                    form.repositories.clone(),
                    form.worker_device.clone(),
                );
                let (max_concurrent_workers, max_followups) =
                    (form.max_workers, form.max_followups);
                let purpose = values[1].clone();
                let avatar = Some(form.avatar.clone());
                let avatar_image = form.image.clone();
                match latest {
                    Some(zot) => ZotAction::Configure {
                        config: ZotConfig {
                            name,
                            purpose,
                            home_device_id,
                            repositories,
                            worker_device_id,
                            max_concurrent_workers,
                            max_followups,
                            avatar,
                            avatar_image,
                            ..zot.config
                        },
                    },
                    None => {
                        after = ZotAfter::Created;
                        ZotAction::Create {
                            config: ZotConfig {
                                name,
                                purpose,
                                home_device_id,
                                repositories,
                                // The coordinator's composer changes this later.
                                coordinator_model: self.zot_default_model(cx),
                                worker_device_id,
                                max_concurrent_workers,
                                max_followups,
                                avatar,
                                avatar_image,
                            },
                        }
                    }
                }
            }
            Kind::File(file) => {
                let (path, content) = if file.version == 0 {
                    (values[0].trim().to_owned(), values[1].clone())
                } else {
                    (file.path.clone(), values[0].clone())
                };
                if path.is_empty() {
                    error(self, "Name the document", cx);
                    return;
                }
                ZotAction::WriteFile {
                    path,
                    content,
                    expected_version: file.version,
                }
            }
            Kind::Decision(decision) => {
                let answer = values[0].trim().to_owned();
                if answer.is_empty() {
                    error(self, "Write an answer first", cx);
                    return;
                }
                let mut decision = decision.clone();
                decision.answer = Some(answer);
                ZotAction::Decide { decision }
            }
            Kind::Thread(Some(chat_id)) => {
                let text = values[0].trim().to_owned();
                if text.is_empty() {
                    error(self, "Write a message first", cx);
                    return;
                }
                ZotAction::MessageWorker {
                    chat_id: chat_id.clone(),
                    text,
                    automatic: false,
                }
            }
            Kind::Thread(None) => {
                let objective = values[0].trim().to_owned();
                if objective.is_empty() {
                    error(self, "Describe what the thread should do", cx);
                    return;
                }
                let repository = form
                    .repositories
                    .iter()
                    .find(|r| Some(&r.space_id) == form.space.as_ref())
                    .or_else(|| {
                        form.repositories
                            .iter()
                            .find(|r| r.device_id == form.worker_device)
                    })
                    .or(form.repositories.first());
                let Some(repository) = repository else {
                    error(self, "Add a repository to this Zapper first", cx);
                    return;
                };
                let zot = latest.as_ref().or(form.zot.as_ref()).unwrap();
                let base = values[1].trim();
                ZotAction::Delegate {
                    issue_id: None,
                    worker: ZotWorker {
                        chat_id: String::new(),
                        device_id: repository.device_id.clone(),
                        space_id: repository.space_id.clone(),
                        objective,
                        followups: 0,
                        queue_position: None,
                    },
                    model: self.zot_thread_model(zot, cx),
                    base: if base.is_empty() {
                        "HEAD".into()
                    } else {
                        base.to_owned()
                    },
                }
            }
        };
        let id = form.id.clone();
        self.send_zot_command(id, revision, action, after, cx);
    }

    pub(super) fn send_zot_command(
        &mut self,
        id: String,
        expected_revision: Option<u32>,
        action: ZotAction,
        after: ZotAfter,
        cx: &mut Context<Self>,
    ) {
        let from_issue_panel = std::mem::take(&mut self.zot_ui.issue_command);
        if self.zot_ui.pending {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let home = self
            .state
            .read(cx)
            .zots
            .iter()
            .find(|z| z.id == id)
            .map(|z| z.config.home_device_id.clone())
            .or_else(|| {
                self.zot_ui
                    .page
                    .as_ref()
                    .filter(|page| page.id == id)
                    .map(|page| page.home.clone())
            });
        let mut command = ZotCommand {
            command_id: uuid::Uuid::new_v4().to_string(),
            zot_id: id,
            expected_revision,
            action,
        };
        if let ZotAction::Delegate { worker, .. } = &mut command.action {
            worker.chat_id = format!("zot-worker-{}", command.command_id);
        }
        if let Some(previous) = &self.zot_ui.retry {
            let mut comparable = command.clone();
            comparable.command_id = previous.command_id.clone();
            comparable.expected_revision = previous.expected_revision;
            if let ZotAction::Delegate { worker, .. } = &mut comparable.action {
                worker.chat_id = format!("zot-worker-{}", previous.command_id);
            }
            if serde_json::to_value(&comparable).ok() == serde_json::to_value(previous).ok() {
                command = previous.clone();
            }
        }
        self.zot_ui.retry = Some(command.clone());
        let mut params = serde_json::to_value(command).unwrap();
        if let Some(home) = home {
            params["targetDeviceId"] = serde_json::json!(home);
        }
        self.zot_ui.pending = true;
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(zuse_protocol::control::ZOT_COMMAND, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.zot_ui.pending = false;
                match result {
                    Ok(value) => {
                        this.zot_ui.retry = None;
                        if let Some(panel) =
                            this.zot_ui.issues.as_ref().filter(|_| from_issue_panel)
                        {
                            panel.update(cx, |p, cx| p.committed(cx));
                        }
                        this.zot_ui.loaded = None;
                        let coordinator = serde_json::from_value::<Option<Zot>>(value)
                            .ok()
                            .flatten()
                            .map(|zot| zot.coordinator_chat_id);
                        this.finish_zot_command(after, coordinator, cx);
                    }
                    Err(error) => {
                        // The engine answered: nothing was written, so the
                        // next attempt is a new command at the current revision.
                        if !zuse_protocol::zots::command_outcome_unknown(&error.to_string()) {
                            this.zot_ui.retry = None;
                        }
                        if let Some(panel) =
                            this.zot_ui.issues.as_ref().filter(|_| from_issue_panel)
                        {
                            panel.update(cx, |p, cx| p.failed(error.to_string(), cx));
                        }
                        this.show_notice(crate::toast::ToastKind::Error, error.to_string(), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_zot_command(
        &mut self,
        after: ZotAfter,
        coordinator: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match after {
            ZotAfter::Stay => {}
            ZotAfter::CloseDialog => self.zot_ui.dialog = None,
            ZotAfter::LeavePage => self.leave_zot_page(cx),
            ZotAfter::Created => {
                self.zot_ui.page = None;
                if let Some(chat) = coordinator.filter(|_| matches!(self.route, Route::Zot)) {
                    let entry = NavEntry::Chat(chat);
                    self.nav.replace(entry.clone());
                    self.apply_nav(entry, cx);
                }
            }
            ZotAfter::OpenCoordinator => {
                if let Some(chat) = coordinator {
                    self.open_chat(chat, cx);
                }
            }
        }
    }

    fn zot_avatar_picker(
        &self,
        current: &ZotAvatar,
        image: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Picking a robot or colour means "use the robot", so it also clears
        // a picture; the robot stays the fallback either way.
        let set = |avatar: ZotAvatar| {
            cx.listener(move |this: &mut Shell, _: &gpui::ClickEvent, _, cx| {
                if let Some(form) = this.zot_ui.page.as_mut() {
                    form.avatar = avatar.clone();
                    form.image = None;
                }
                cx.notify();
            })
        };
        let mut mascots = div().flex().flex_wrap().gap(px(6.0));
        for mascot in zot_mascots::MASCOTS {
            let avatar = ZotAvatar {
                mascot: mascot.id.into(),
                color: current.color.clone(),
            };
            let active = !image && mascot.id == current.mascot;
            let label = format!("{}: {}", mascot.name, mascot.about);
            mascots = mascots.child(
                div()
                    .id(SharedString::from(format!("zot-mascot-{}", mascot.id)))
                    .role(gpui::Role::RadioButton)
                    .aria_label(SharedString::from(label.clone()))
                    .tip(label)
                    .size(px(42.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.0))
                    .border_2()
                    .cursor_pointer()
                    .when(active, |d| d.border_color(theme.text_strong))
                    .when(!active, |d| {
                        d.border_color(gpui::transparent_black())
                            .hover(|d| d.border_color(theme.border_strong))
                    })
                    .child(zot_mascot(&ZapperFace::mascot(avatar.clone()), 32.0))
                    .on_click(set(avatar.clone())),
            );
        }
        let mut colors = div().flex().items_center().gap(px(6.0));
        for color in zot_mascots::COLORS {
            let active = !image && color.id == current.color;
            colors = colors.child(
                div()
                    .id(SharedString::from(format!("zot-color-{}", color.id)))
                    .role(gpui::Role::RadioButton)
                    .aria_label(color.id)
                    .size(px(22.0))
                    .p(px(3.0))
                    .rounded_full()
                    .border_1()
                    .cursor_pointer()
                    .border_color(if active {
                        theme.text_strong
                    } else {
                        gpui::transparent_black()
                    })
                    .child(div().size_full().rounded_full().bg(gpui::rgb(color.body)))
                    .on_click(set(ZotAvatar {
                        mascot: current.mascot.clone(),
                        color: color.id.into(),
                    })),
            );
        }
        let picture = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                widgets::ghost_action(theme)
                    .id("zot-upload-image")
                    .role(gpui::Role::Button)
                    .child(
                        icon(icons::UPLOAD)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    )
                    .child(if image {
                        "Replace image"
                    } else {
                        "Upload image"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.upload_zot_picture(cx))),
            )
            .when(image, |d| {
                d.child(
                    widgets::ghost_action(theme)
                        .id("zot-remove-image")
                        .role(gpui::Role::Button)
                        .child(
                            icon(icons::CLOSE)
                                .size(px(13.0))
                                .text_color(theme.text_muted),
                        )
                        .child("Use robot")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(form) = this.zot_ui.page.as_mut() {
                                form.image = None;
                            }
                            cx.notify();
                        })),
                )
            });
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(mascots)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(colors)
                    .child(
                        widgets::ghost_action(theme)
                            .id("zot-shuffle")
                            .role(gpui::Role::Button)
                            .child(
                                icon(icons::REFRESH)
                                    .size(px(13.0))
                                    .text_color(theme.text_muted),
                            )
                            .child("Shuffle")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(form) = this.zot_ui.page.as_mut() {
                                    form.avatar = zot_mascots::default_avatar(
                                        &uuid::Uuid::new_v4().to_string(),
                                    );
                                    form.image = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(picture)
            .into_any_element()
    }

    fn zot_repo_choices(&self, cx: &App) -> Vec<RepoChoice> {
        let state = self.state.read(cx);
        repo_choices(&state.spaces, &state.devices)
    }

    /// The choices the repository search currently shows, in list order.
    fn zot_visible_repos(&self, cx: &App) -> Vec<RepoChoice> {
        let Some(form) = self.zot_ui.page.as_ref() else {
            return vec![];
        };
        let query = form
            .repo_search
            .as_ref()
            .map(|input| input.read(cx).text().to_owned())
            .unwrap_or_default();
        let choices = self.zot_repo_choices(cx);
        filter_repo_choices(&query, &choices)
            .into_iter()
            .map(|index| choices[index].clone())
            .collect()
    }

    fn toggle_zot_repository(&mut self, choice: &RepoChoice, cx: &mut Context<Self>) {
        if let Some(form) = self.zot_ui.page.as_mut() {
            toggle_repository(&mut form.repositories, choice);
        }
        cx.notify();
    }

    /// Arrows move the highlight, Enter switches the highlighted repository
    /// and Escape clears the search. Other keys go on to the form.
    fn zot_repo_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let key = popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        );
        let visible = self.zot_visible_repos(cx);
        let Some(form) = self.zot_ui.page.as_mut() else {
            return;
        };
        match key {
            popover::MenuKey::Up | popover::MenuKey::Down => {
                let delta = if key == popover::MenuKey::Up { -1 } else { 1 };
                form.repo_active = popover::menu_step(form.repo_active, visible.len(), delta);
                if let Some(active) = form.repo_active {
                    form.repo_scroll.scroll_to_item(active);
                }
            }
            popover::MenuKey::Enter => {
                let Some(choice) = form.repo_active.and_then(|i| visible.get(i)).cloned() else {
                    return;
                };
                self.toggle_zot_repository(&choice, cx);
            }
            popover::MenuKey::Escape => {
                let Some(input) = form.repo_search.clone() else {
                    return;
                };
                if input.read(cx).text().is_empty() && form.repo_active.is_none() {
                    return;
                }
                input.update(cx, |input, cx| input.set_text("", cx));
                form.repo_active = None;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn zot_repository_picker(
        &self,
        selected: &[ZotRepository],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let choices = self.zot_repo_choices(cx);
        if choices.is_empty() {
            return widgets::section_card(theme)
                .mt(px(0.0))
                .p(px(16.0))
                .text_size(crate::typography::text_sm())
                .text_color(theme.text_muted)
                .child("No projects yet. Add one from GitHub or a folder on this device.")
                .into_any_element();
        }
        let Some(form) = self.zot_ui.page.as_ref() else {
            return div().into_any_element();
        };
        let Some(search) = form.repo_search.clone() else {
            return div().into_any_element();
        };
        let active = form.repo_active;
        let scroll = form.repo_scroll.clone();
        let query = search.read(cx).text().to_owned();
        let visible = self.zot_visible_repos(cx);

        let search_row = catalog_ui::search_field(theme, None, search.into_any_element())
            .h(px(32.0))
            .when(!query.is_empty(), |field| {
                field.child(
                    div()
                        .id("zot-repo-search-clear")
                        .role(gpui::Role::Button)
                        .aria_label("Clear search")
                        .flex_none()
                        .size(px(18.0))
                        .mr(px(-4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|d| d.bg(theme.element_hover))
                        .child(
                            icon(icons::CLOSE)
                                .size(px(11.0))
                                .text_color(theme.text_muted),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(form) = this.zot_ui.page.as_mut() {
                                if let Some(input) = form.repo_search.clone() {
                                    input.update(cx, |input, cx| input.set_text("", cx));
                                }
                                form.repo_active = None;
                            }
                            cx.notify();
                        })),
                )
            });

        // Selected repositories, in the order they were switched on.
        let chips = (!selected.is_empty()).then(|| {
            let mut row = div().flex().flex_wrap().gap(px(6.0));
            for repository in selected {
                let choice = choices
                    .iter()
                    .find(|c| c.space_id == repository.space_id)
                    .cloned();
                let label = choice
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "Removed project".into());
                let device = choice.as_ref().and_then(|c| c.device.clone());
                let space_id = repository.space_id.clone();
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(26.0))
                        .pl(px(10.0))
                        .pr(px(4.0))
                        .rounded_full()
                        .border_1()
                        .border_color(theme.border_strong)
                        .bg(theme.element_active)
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_strong)
                        .child(label.clone())
                        .children(
                            device.map(|device| div().text_color(theme.text_faint).child(device)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("zot-repo-chip-{space_id}")))
                                .role(gpui::Role::Button)
                                .aria_label(SharedString::from(format!("Remove {label}")))
                                .size(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .hover(|d| d.bg(theme.element_hover))
                                .child(
                                    icon(icons::CLOSE)
                                        .size(px(10.0))
                                        .text_color(theme.text_muted),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(form) = this.zot_ui.page.as_mut() {
                                        form.repositories.retain(|r| r.space_id != space_id);
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            row
        });

        let list = if visible.is_empty() {
            widgets::section_card(theme)
                .mt(px(0.0))
                .px(px(16.0))
                .py(px(20.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(crate::typography::text_sm())
                        .text_color(theme.text_muted)
                        .child(format!(
                            "No repositories match \u{201c}{}\u{201d}",
                            query.trim()
                        )),
                )
                .child(hint(theme, "Try part of the name or folder path."))
                .into_any_element()
        } else {
            let spaces = self.state.read(cx).spaces.clone();
            let mut rows = div()
                .id("zot-repo-list")
                .role(gpui::Role::List)
                .max_h(px(REPO_LIST_HEIGHT))
                .p(px(4.0))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .overflow_y_scroll()
                .track_scroll(&scroll);
            for (index, choice) in visible.into_iter().enumerate() {
                let checked = selected.iter().any(|r| r.space_id == choice.space_id);
                let highlighted = active == Some(index);
                // The path only when the name alone may not say which
                // checkout it is; the device when there are several.
                let detail = match (&choice.device, choice.hosted) {
                    (Some(device), _) => Some(device.clone()),
                    (None, false) => Some(choice.path.clone()),
                    (None, true) => None,
                };
                let space = spaces.iter().find(|s| s.id == choice.space_id);
                let artwork = div()
                    .size(px(16.0))
                    .flex_none()
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .child(self.project_artwork(space, false, cx));
                rows = rows.child(
                    div()
                        .id(SharedString::from(format!("zot-repo-{}", choice.space_id)))
                        .role(gpui::Role::CheckBox)
                        .aria_label(SharedString::from(choice.name.clone()))
                        .aria_selected(checked)
                        .h(px(REPO_ROW_HEIGHT))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(highlighted, |d| d.bg(theme.element_hover))
                        .hover(|d| d.bg(theme.element_hover))
                        .child(artwork)
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(320.0))
                                .truncate()
                                .text_size(crate::typography::text_sm())
                                .text_color(if checked {
                                    theme.text_strong
                                } else {
                                    theme.text
                                })
                                .child(choice.name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::text_xs())
                                .text_color(theme.text_faint)
                                .children(detail),
                        )
                        .child(widgets::select_check(theme, checked))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.zot_ui.page.as_mut() {
                                form.repo_active = Some(index);
                            }
                            this.toggle_zot_repository(&choice, cx);
                        })),
                );
            }
            widgets::section_card(theme)
                .mt(px(0.0))
                .overflow_hidden()
                .child(rows)
                .into_any_element()
        };

        div()
            .id("zot-repo-picker")
            .flex()
            .flex_col()
            .gap(px(LABEL_GAP))
            .on_key_down(
                cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| this.zot_repo_key(event, cx)),
            )
            .child(search_row)
            .children(chips)
            .child(list)
            .into_any_element()
    }

    /// A − value + control for a bounded count.
    fn zot_stepper(
        &self,
        id: &'static str,
        value: u32,
        range: std::ops::RangeInclusive<u32>,
        set: fn(&mut Form, u32),
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = |suffix: &'static str, glyph: &'static str, delta: i32, enabled: bool| {
            let range = range.clone();
            div()
                .id(SharedString::from(format!("{id}-{suffix}")))
                .role(gpui::Role::Button)
                .aria_label(if delta < 0 { "Fewer" } else { "More" })
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .text_size(crate::typography::text_sm())
                .text_color(theme.text_muted)
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(|d| d.bg(theme.element_hover).text_color(theme.text_strong))
                })
                .when(!enabled, |d| d.opacity(0.4))
                .child(glyph)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.zot_ui.page.as_mut() {
                        set(form, step_limit(value, delta, &range));
                    }
                    cx.notify();
                }))
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .rounded(px(7.0))
            .bg(catalog_ui::control_fill(theme))
            .child(button("less", "\u{2212}", -1, value > *range.start()))
            .child(
                div()
                    .w(px(28.0))
                    .text_center()
                    .text_size(crate::typography::text_sm())
                    .text_color(theme.text_strong)
                    .child(value.to_string()),
            )
            .child(button("more", "+", 1, value < *range.end()))
            .into_any_element()
    }

    fn zot_device_chips(
        &self,
        selected: &str,
        home: bool,
        locked: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let devices = self.state.read(cx).devices.clone();
        let mut chips = div().flex().flex_wrap().justify_end().gap(px(4.0));
        for device in devices {
            let active = selected == device.id;
            let id = device.id.clone();
            chips = chips.child(
                chip(
                    format!("zot-device-{home}-{id}"),
                    device.name,
                    active,
                    theme,
                )
                .when(locked && !active, |d| d.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.zot_ui.page.as_mut() {
                        if !home {
                            form.worker_device = id.clone();
                        } else if form.zot.is_none() {
                            form.home = id.clone();
                        }
                    }
                    cx.notify();
                })),
            );
        }
        chips.into_any_element()
    }

    /// How the coordinator's model reads in Advanced.
    fn zot_model_label(&self, zot: Option<&Zot>, cx: &App) -> String {
        let model = match zot {
            Some(zot) => self.zot_thread_model(zot, cx),
            None => self.zot_default_model(cx),
        };
        let harness = zuse_protocol::HarnessId::try_from(model.harness.clone())
            .map(|h| crate::pickers::harness_display_name(h).to_owned())
            .unwrap_or(model.harness);
        match model.model {
            Some(id) => {
                let label = crate::pickers::known_model_label(&id).unwrap_or(id);
                format!("{harness} · {label}")
            }
            None => harness,
        }
    }

    /// The fields shared by the New Zot page and the panel editor: name,
    /// what it should do, repositories, then Advanced (collapsed).
    fn zot_form_fields(
        &mut self,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = Theme::of(cx).clone();
        let spaces = self.state.read(cx).spaces.clone();
        let form = self.zot_ui.page.as_mut().unwrap();
        if std::mem::take(&mut form.focus_pending) {
            window.focus(&form.inputs[0].focus_handle(cx), cx);
        }
        if let Some(known) = &form.known_spaces {
            let added: Vec<_> = spaces.iter().filter(|s| !known.contains(&s.id)).collect();
            if !added.is_empty() {
                form.repositories
                    .extend(added.into_iter().map(|space| ZotRepository {
                        space_id: space.id.clone(),
                        device_id: space.device_id.clone(),
                    }));
                form.known_spaces = None;
            }
        }
        let avatar = form.avatar.clone();
        let face = ZapperFace {
            avatar: avatar.clone(),
            image: form.image.clone().map(SharedString::from),
            mood: Default::default(),
        };
        let name = form.inputs[0].clone();
        let purpose = form.inputs[1].clone();
        let editing = form.zot.is_some();
        let selected = form.repositories.clone();
        let (home, worker_device) = (form.home.clone(), form.worker_device.clone());
        let (advanced_open, max_workers, max_followups) =
            (form.advanced_open, form.max_workers, form.max_followups);
        let zot = form.zot.clone();
        let multiple_devices = self.state.read(cx).devices.len() > 1;
        let identity = div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .child(zot_mascot(&face, if compact { 48.0 } else { 64.0 }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(labelled(&theme, "Name", name, 36.0, cx)),
            );
        let character = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(widgets::field_label(&theme, "Character"))
            .child(self.zot_avatar_picker(&avatar, face.image.is_some(), &theme, cx));
        let purpose = div()
            .flex()
            .flex_col()
            .gap(px(LABEL_GAP))
            .child(field_heading(
                &theme,
                "What should it do?",
                None,
                Some(hint(&theme, "Optional").into_any_element()),
            ))
            .child(text_box(purpose, 96.0, cx));
        let add = widgets::action_button(&theme, ActionTone::Outlined)
            .id("zot-add-project")
            .role(gpui::Role::Button)
            .child(icon(icons::PLUS).size(px(12.0)))
            .child(if compact { "Add" } else { "Add project" })
            .on_click(cx.listener(|this, _, _, cx| {
                let known = this
                    .state
                    .read(cx)
                    .spaces
                    .iter()
                    .map(|s| s.id.clone())
                    .collect();
                if let Some(form) = this.zot_ui.page.as_mut() {
                    form.known_spaces = Some(known);
                }
                this.open_project_setup(None, cx);
            }));
        let repositories = div()
            .flex()
            .flex_col()
            .gap(px(LABEL_GAP))
            .child(field_heading(
                &theme,
                "Repositories",
                Some("Threads this Zapper starts work in the ones you pick."),
                Some(add.into_any_element()),
            ))
            .child(self.zot_repository_picker(&selected, &theme, cx));

        let row = |title: &'static str, detail: &'static str, control: AnyElement, first| {
            widgets::card_row(&theme, first)
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(widgets::row_title(&theme, title))
                        .child(hint(&theme, detail)),
                )
                .child(control)
        };
        let model = self.zot_model_label(zot.as_ref(), cx);
        let summary = format!("{model} · {max_workers} at once · {max_followups} follow-ups");
        let toggle = div()
            .id("zot-advanced")
            .role(gpui::Role::Button)
            .aria_expanded(advanced_open)
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .child(
                icon(if advanced_open {
                    icons::ALT_ARROW_DOWN
                } else {
                    icons::ALT_ARROW_RIGHT
                })
                .size(px(13.0))
                .flex_none()
                .text_color(theme.text_muted),
            )
            .child(widgets::field_label(&theme, "Advanced"))
            .when(!advanced_open, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(hint(&theme, summary)),
                )
            })
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(form) = this.zot_ui.page.as_mut() {
                    form.advanced_open = !form.advanced_open;
                }
                cx.notify();
            }));
        let advanced = advanced_open.then(|| {
            let mut card = widgets::section_card(&theme).mt(px(0.0)).child(row(
                "Model",
                "Change it later from the Zapper's chat",
                div()
                    .flex_none()
                    .text_size(crate::typography::text_sm())
                    .text_color(theme.text_muted)
                    .child(model)
                    .into_any_element(),
                true,
            ));
            if multiple_devices {
                let host = self.zot_device_chips(&home, true, editing, &theme, cx);
                let workers = self.zot_device_chips(&worker_device, false, false, &theme, cx);
                card = card
                    .child(row("Runs on", "Where the Zapper itself lives", host, false))
                    .child(row(
                        "Threads run on",
                        "Default device for new threads",
                        workers,
                        false,
                    ));
            }
            let workers = self.zot_stepper(
                "zot-max-workers",
                max_workers,
                MAX_WORKERS,
                |form, value| form.max_workers = value,
                &theme,
                cx,
            );
            let followups = self.zot_stepper(
                "zot-max-followups",
                max_followups,
                MAX_FOLLOWUPS,
                |form, value| form.max_followups = value,
                &theme,
                cx,
            );
            card.child(row(
                "Threads at once",
                "How many threads it runs in parallel",
                workers,
                false,
            ))
            .child(row(
                "Follow-ups per thread",
                "Automatic nudges before a thread waits for you",
                followups,
                false,
            ))
        });
        let advanced = div()
            .flex()
            .flex_col()
            .gap(px(LABEL_GAP))
            .child(toggle)
            .children(advanced);
        div()
            .flex()
            .flex_col()
            .gap(px(if compact { 20.0 } else { 28.0 }))
            .child(identity)
            .child(character)
            .child(purpose)
            .child(repositories)
            .child(advanced)
    }

    pub(super) fn render_zot_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.zot_ui.page.is_none() {
            self.zot_ui.page = Some(self.build_zot_form(None, Kind::Config, false, cx));
        }
        let theme = Theme::of(cx).clone();
        let saving = self.zot_ui.pending;
        let page_scroll = self.zot_ui.page.as_ref().map(|f| f.page_scroll.clone());
        let column = || widgets::page_column().max_w(px(640.0));
        let header = div()
            .flex()
            .flex_col()
            .child(widgets::page_header(&theme, "New Zapper", None))
            .child(widgets::page_subtitle(
                &theme,
                "A Zapper is your captain: it runs work across your repositories, asks when it needs a decision and keeps its notes between conversations.",
            ));
        let fields = self.zot_form_fields(false, window, cx);
        let submit = if saving {
            "Creating…"
        } else {
            "Create Zapper"
        };
        div()
            .id("zot-page")
            .debug_selector(|| "zot-page".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .child(
                div()
                    .id("zot-page-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when_some(page_scroll, |d, scroll| d.track_scroll(&scroll))
                    .child(
                        column()
                            .pt(px(16.0))
                            .pb(px(32.0))
                            .gap(px(24.0))
                            .child(header)
                            .child(fields),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        column()
                            .pt(px(12.0))
                            .pb(px(12.0))
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                widgets::text_action(&theme, ActionTone::Quiet, "Cancel")
                                    .id("zot-cancel")
                                    .role(gpui::Role::Button)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.leave_zot_page(cx)),
                                    ),
                            )
                            .child(
                                popover::btn_primary(&theme, submit)
                                    .id("zot-save")
                                    .role(gpui::Role::Button)
                                    .when(saving, |d| d.opacity(0.6))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_zot_form(false, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Editing lives in the right panel, beside the conversation it configures.
    pub(super) fn render_zot_editor(
        &mut self,
        zot: &Zot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let saving = self.zot_ui.pending;
        let fields = self.zot_form_fields(true, window, cx);
        let manage = self.zot_manage(zot, &theme, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("zot-editor-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(18.0))
                    .py(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(22.0))
                    .child(fields)
                    .child(manage),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .p(px(10.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        widgets::text_action(&theme, ActionTone::Quiet, "Cancel")
                            .id("zot-editor-cancel")
                            .role(gpui::Role::Button)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zot_ui.page = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(
                            &theme,
                            if saving { "Saving…" } else { "Save changes" },
                        )
                        .id("zot-editor-save")
                        .role(gpui::Role::Button)
                        .when(saving, |d| d.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_zot_form(false, cx))),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_zot_dialog(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let saving = self.zot_ui.pending;
        let form = self.zot_ui.dialog.as_mut()?;
        if std::mem::take(&mut form.focus_pending) {
            window.focus(&form.inputs[0].focus_handle(cx), cx);
        }
        let inputs = form.inputs.clone();
        let (title, submit, width): (String, &str, f32) = match &form.kind {
            Kind::File(file) if file.version == 0 => ("New document".into(), "Save", 560.0),
            Kind::File(file) => (file.path.clone(), "Save", 560.0),
            Kind::Decision(_) => ("Answer".into(), "Send answer", 420.0),
            Kind::Thread(Some(_)) => ("Message thread".into(), "Send", 420.0),
            Kind::Thread(None) => ("New thread".into(), "Start thread", 460.0),
            Kind::Config => (String::new(), "", 0.0),
        };
        let question = match &form.kind {
            Kind::Decision(decision) => Some(decision.question.clone()),
            _ => None,
        };
        let mut body = div().mt(px(14.0)).flex().flex_col().gap(px(12.0));
        body = match &form.kind {
            Kind::File(file) if file.version == 0 => body
                .child(labelled(&theme, "Name", inputs[0].clone(), 36.0, cx))
                .child(labelled(&theme, "Content", inputs[1].clone(), 260.0, cx)),
            Kind::File(_) => body.child(text_box(inputs[0].clone(), 300.0, cx)),
            Kind::Thread(None) => body
                .child(labelled(&theme, "Task", inputs[0].clone(), 84.0, cx))
                .child(labelled(&theme, "Start from", inputs[1].clone(), 36.0, cx)),
            _ => body.child(text_box(inputs[0].clone(), 84.0, cx)),
        };
        let projects = matches!(form.kind, Kind::Thread(None)) && form.repositories.len() > 1;
        if projects {
            let state = self.state.read(cx);
            let names: Vec<(String, String)> = form
                .repositories
                .iter()
                .filter_map(|repository| {
                    let space = state.spaces.iter().find(|s| s.id == repository.space_id)?;
                    let name = space
                        .name
                        .clone()
                        .or_else(|| space.path.rsplit('/').next().map(str::to_owned))?;
                    Some((space.id.clone(), name))
                })
                .collect();
            let current = form.space.clone().or_else(|| {
                form.repositories
                    .iter()
                    .find(|r| r.device_id == form.worker_device)
                    .map(|r| r.space_id.clone())
            });
            let mut chips = div().flex().flex_wrap().gap(px(4.0));
            for (id, name) in names {
                let active = current.as_ref() == Some(&id);
                chips = chips.child(
                    chip(format!("zot-thread-space-{id}"), name, active, &theme).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.zot_ui.dialog.as_mut() {
                                form.space = Some(id.clone());
                            }
                            cx.notify();
                        }),
                    ),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(widgets::field_label(&theme, "Project"))
                    .child(chips),
            );
        }
        let card = popover::dialog_card(&theme)
            .id("zot-dialog-card")
            .role(gpui::Role::Dialog)
            .aria_label(SharedString::from(title.clone()))
            .w(px(width))
            .child(popover::dialog_title(&theme, &title))
            .when_some(question, |d, question| {
                d.child(
                    div()
                        .mt(px(6.0))
                        .child(popover::dialog_body(&theme, question)),
                )
            })
            .child(body)
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        widgets::text_action(&theme, ActionTone::Quiet, "Cancel")
                            .id("zot-dialog-cancel")
                            .role(gpui::Role::Button)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zot_ui.dialog = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(&theme, submit)
                            .id("zot-dialog-save")
                            .role(gpui::Role::Button)
                            .when(saving, |d| d.opacity(0.6))
                            .on_click(cx.listener(|this, _, _, cx| this.submit_zot_form(true, cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("zot-dialog", viewport, card))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_documents_are_named_but_existing_ones_edit_content() {
        let file = |version| ZotFile {
            path: "NOTES.md".into(),
            version,
            content: "kept".into(),
        };
        assert_eq!(fields(&Kind::File(file(0)), None).len(), 2);
        let existing = fields(&Kind::File(file(2)), None);
        assert_eq!(existing.len(), 1);
        assert_eq!(existing[0].1, "kept");
    }

    fn space(id: &str, device: &str, path: &str) -> zuse_protocol::Space {
        serde_json::from_value(serde_json::json!({
            "id": id, "deviceId": device, "path": path,
            "createdAt": "2026-09-08T00:00:00Z"
        }))
        .unwrap()
    }

    fn device(id: &str, name: &str) -> zuse_protocol::Device {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "platform": "linux", "lastSeenAt": null
        }))
        .unwrap()
    }

    fn choices() -> Vec<RepoChoice> {
        let mut renamed = space("site", "local", "/Users/sam/code/www");
        renamed.name = Some("Marketing Site".into());
        let mut hosted = space("zuse", "local", "/Users/sam/code/zuse-checkout");
        hosted.remote_repository = Some("github.com/swarajbachu/zuse".into());
        repo_choices(
            &[
                hosted,
                space("web", "local", "/Users/sam/code/acme-web"),
                space("api", "local", "/Users/sam/code/acme-api"),
                renamed,
                space("infra", "remote", "/srv/repos/Infra/"),
            ],
            &[
                device("local", "Studio Mac"),
                device("remote", "Build server"),
            ],
        )
    }

    fn names(query: &str) -> Vec<String> {
        let choices = choices();
        filter_repo_choices(query, &choices)
            .into_iter()
            .map(|i| choices[i].name.clone())
            .collect()
    }

    #[test]
    fn repository_search_matches_name_anywhere_ignoring_case() {
        assert_eq!(names("API"), ["acme-api"]);
        assert_eq!(names("site"), ["Marketing Site"]);
        assert_eq!(names("KETING"), ["Marketing Site"]);
        // Prefix matches come first, then the rest in list order.
        assert_eq!(names("acme"), ["acme-web", "acme-api"]);
    }

    #[test]
    fn repository_search_matches_path() {
        assert_eq!(names("/users/sam/code/www"), ["Marketing Site"]);
        assert_eq!(names("srv/repos"), ["Infra"]);
        assert_eq!(names("sam/code").len(), 4);
        // A hosted project is still found by its folder.
        assert_eq!(names("zuse-checkout"), ["swarajbachu/zuse"]);
    }

    #[test]
    fn repository_search_empty_query_lists_all_and_misses_list_none() {
        assert_eq!(names("  ").len(), 5);
        assert!(names("nothing-here").is_empty());
    }

    #[test]
    fn hosted_projects_are_named_owner_slash_repo() {
        assert_eq!(names("SWARAJ"), ["swarajbachu/zuse"]);
        assert_eq!(names("bachu/zu"), ["swarajbachu/zuse"]);
        let all = choices();
        assert!(all[0].hosted);
        assert!(!all[1].hosted);
        assert_eq!(
            remote_name("github.com/acme/app.git").as_deref(),
            Some("acme/app")
        );
        assert_eq!(remote_name("github.com/acme"), None);
    }

    #[test]
    fn repository_choices_name_devices_only_when_several() {
        let all = choices();
        assert_eq!(all[4].name, "Infra");
        assert_eq!(all[4].device.as_deref(), Some("Build server"));
        assert_eq!(all[1].device.as_deref(), Some("Studio Mac"));
        let one = repo_choices(
            &[space("web", "local", "/a/web")],
            &[device("local", "Mac")],
        );
        assert_eq!(one[0].device, None);
    }

    #[test]
    fn repositories_toggle_on_and_off() {
        let all = choices();
        let mut selected = vec![];
        assert!(toggle_repository(&mut selected, &all[2]));
        assert!(toggle_repository(&mut selected, &all[4]));
        assert_eq!(
            selected,
            vec![
                ZotRepository {
                    space_id: "api".into(),
                    device_id: "local".into()
                },
                ZotRepository {
                    space_id: "infra".into(),
                    device_id: "remote".into()
                },
            ]
        );
        assert!(!toggle_repository(&mut selected, &all[2]));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].space_id, "infra");
    }

    #[test]
    fn limits_stay_in_the_engine_range() {
        assert_eq!(step_limit(1, -1, &MAX_WORKERS), 1);
        assert_eq!(step_limit(32, 1, &MAX_WORKERS), 32);
        assert_eq!(step_limit(4, 1, &MAX_WORKERS), 5);
        assert_eq!(step_limit(0, -1, &MAX_FOLLOWUPS), 0);
    }

    #[test]
    fn purpose_is_optional_with_a_hint() {
        let config = fields(&Kind::Config, None);
        assert_eq!(config[1].0, PURPOSE_HINT);
        assert!(config[1].1.is_empty());
    }
}
