//! Project entry point. Filesystem navigation stays in the existing folder palette.
use super::*;
use gpui::{Div, Stateful};
use zuse_protocol::{Device, Space, project_creation::*};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Choose,
    Clone,
    Create,
}

pub(super) struct ProjectSetup {
    generation: String,
    device: Option<Device>,
    mode: Mode,
    /// Cloning: a repository or URL is chosen and the destination is next.
    clone_confirm: bool,
    active: usize,
    /// A row is highlighted by the keyboard or pointer; until then Enter
    /// completes the step.
    navigating: bool,
    query: Entity<ComposerInput>,
    name: Entity<ComposerInput>,
    pub(super) parent: Entity<ComposerInput>,
    url: Entity<ComposerInput>,
    template: ProjectTemplate,
    publish: bool,
    github: Loadable<GithubProjectStatus>,
    repositories: Loadable<GithubProjectRepositories>,
    busy: bool,
    error: Option<String>,
    focus_pending: bool,
    focus: gpui::FocusHandle,
    scroll: gpui::ScrollHandle,
    _events: Vec<Subscription>,
    parent_task: Option<Task<()>>,
    github_task: Option<Task<()>>,
}

impl Shell {
    pub(super) fn open_project_setup(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        self.project_parent_setup = None;
        self.open_add_space_browser(cx);
        let state = self.state.read(cx);
        let device_id = device.or_else(|| state.local_device_id.clone());
        let device = device_id.and_then(|id| {
            state
                .devices
                .iter()
                .find(|d| d.id == id)
                .cloned()
                .or_else(|| {
                    Some(Device {
                        id,
                        name: "Connected computer".into(),
                        platform: String::new(),
                        last_seen_at: None,
                        created_at: None,
                        version: None,
                        cursor_sdk_version: None,
                        capabilities: Vec::new(),
                    })
                })
        });
        let query = cx.new(|cx| {
            ComposerInput::with_context("Find a project or action…", "PaletteSearch", cx)
        });
        let name = cx.new(|cx| {
            ComposerInput::with_context("Project name", "ProjectForm", cx).with_single_line()
        });
        let parent = cx.new(|cx| {
            ComposerInput::with_context("Parent folder", "ProjectForm", cx).with_single_line()
        });
        let url = cx.new(|cx| {
            ComposerInput::with_context(
                "https://github.com/owner/repository.git",
                "ProjectForm",
                cx,
            )
            .with_single_line()
        });
        let events = [&query, &name, &parent, &url]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this: &mut Shell, _, event, cx| {
                    if matches!(event, ComposerInputEvent::Edited) {
                        if let Some(setup) = &mut this.project_setup {
                            setup.active = 0;
                            setup.navigating = false;
                            setup.error = None;
                        }
                        cx.notify();
                    }
                })
            })
            .collect();
        self.project_setup = Some(ProjectSetup {
            generation: uuid::Uuid::new_v4().to_string(),
            device,
            mode: Mode::Choose,
            clone_confirm: false,
            active: 0,
            navigating: false,
            query,
            name,
            parent,
            url,
            template: ProjectTemplate::Empty,
            publish: false,
            github: Loadable::Idle,
            repositories: Loadable::Idle,
            busy: false,
            error: None,
            focus_pending: true,
            focus: cx.focus_handle(),
            scroll: gpui::ScrollHandle::new(),
            _events: events,
            parent_task: None,
            github_task: None,
        });
        self.load_project_parent(cx);
        cx.notify();
    }

    fn project_setup_generation(&mut self, generation: &str) -> Option<&mut ProjectSetup> {
        if let Some(setup) = self
            .project_setup
            .as_mut()
            .filter(|s| s.generation == generation)
        {
            return Some(setup);
        }
        self.project_parent_setup
            .as_mut()
            .filter(|s| s.generation == generation)
    }

    pub(super) fn resume_project_setup(&mut self, mut setup: ProjectSetup, cx: &mut Context<Self>) {
        setup.focus_pending = true;
        self.project_setup = Some(setup);
        cx.notify();
    }

    fn project_params(&self, mut params: serde_json::Value, cx: &App) -> serde_json::Value {
        if let Some(device) = self.project_setup.as_ref().and_then(|s| s.device.as_ref())
            && self.state.read(cx).local_device_id.as_deref() != Some(device.id.as_str())
        {
            params["targetDeviceId"] = device.id.clone().into();
        }
        params
    }

    fn load_project_parent(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.project_params(serde_json::json!({}), cx);
        let Some(setup) = self.project_setup.as_ref() else {
            return;
        };
        let generation = setup.generation.clone();
        if let Some(device) = &setup.device {
            let id = device.id.clone();
            self.state
                .update(cx, |state, cx| state.watch_project_creations(id, cx));
        }
        let task = cx.spawn(async move |this, cx| {
            let result = engine.client().call_as::<String>(DEFAULT_PARENT, params).await;
            this.update(cx, |shell, cx| {
                let Some(setup) = shell.project_setup_generation(&generation) else { return; };
                match result {
                    Ok(parent) if setup.parent.read(cx).is_empty() => setup.parent.update(cx, |input, cx| input.set_text(parent, cx)),
                    Ok(_) => {},
                    Err(_) => setup.error = Some("Could not load the default folder. Enter a parent folder on the selected computer.".into()),
                }
                cx.notify();
            }).ok();
        });
        if let Some(setup) = self.project_setup.as_mut() {
            setup.parent_task = Some(task);
        }
    }

    fn project_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if let Some(setup) = &mut self.project_setup {
            setup.mode = mode;
            setup.clone_confirm = false;
            setup.active = 0;
            setup.navigating = false;
            setup.error = None;
            setup.focus_pending = true;
            setup
                .name
                .update(cx, |input, cx| input.set_placeholder("Project name", cx));
            setup.query.update(cx, |input, cx| {
                input.set_text("", cx);
                input.set_placeholder(
                    if mode == Mode::Clone {
                        "Search your repositories or paste a Git URL…"
                    } else {
                        "Find a project or action…"
                    },
                    cx,
                );
            });
        }
        if mode != Mode::Choose {
            self.load_project_github(1, cx);
        }
        cx.notify();
    }

    fn load_project_github(&mut self, page: u32, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let status_params = self.project_params(serde_json::json!({}), cx);
        let repo_params = self.project_params(serde_json::json!({"page": page}), cx);
        let Some(setup) = self.project_setup.as_mut() else {
            return;
        };
        let generation = setup.generation.clone();
        if page == 1 {
            setup.github = Loadable::Loading;
            setup.repositories = Loadable::Loading;
        }
        setup.github_task = Some(cx.spawn(async move |this, cx| {
            let status = engine
                .client()
                .call_as::<GithubProjectStatus>(GITHUB_STATUS, status_params)
                .await;
            let authenticated = status.as_ref().is_ok_and(|s| s.authenticated);
            if this
                .update(cx, |shell, cx| {
                    if let Some(setup) = shell.project_setup_generation(&generation) {
                        setup.github = match status {
                            Ok(s) => Loadable::Ready(s),
                            Err(_) => Loadable::Error(
                                "GitHub could not be reached on this computer.".into(),
                            ),
                        };
                        if !authenticated {
                            setup.repositories = Loadable::Idle;
                        }
                        cx.notify();
                    }
                })
                .is_err()
            {
                return;
            }
            if !authenticated {
                return;
            }
            let result = engine
                .client()
                .call_as::<GithubProjectRepositories>(GITHUB_REPOS, repo_params)
                .await;
            this.update(cx, |shell, cx| {
                let Some(setup) = shell.project_setup_generation(&generation) else {
                    return;
                };
                setup.repositories = match result {
                    Ok(mut result) => {
                        if page > 1 {
                            if let Loadable::Ready(previous) = &setup.repositories {
                                let mut repos = previous.repositories.clone();
                                repos.extend(result.repositories.into_iter().filter(|r| {
                                    !previous
                                        .repositories
                                        .iter()
                                        .any(|p| p.name_with_owner == r.name_with_owner)
                                }));
                                result.repositories = repos;
                            }
                        }
                        Loadable::Ready(result)
                    }
                    Err(_) => Loadable::Error(
                        "Could not load repositories. Check GitHub access and retry.".into(),
                    ),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn project_browse_parent(&mut self, cx: &mut Context<Self>) {
        if self.project_setup.as_ref().is_some_and(|s| s.busy) {
            return;
        }
        let Some(setup) = self.project_setup.take() else {
            return;
        };
        let device = setup.device.clone();
        let path = setup.parent.read(cx).text().trim().to_owned();
        self.project_parent_setup = Some(setup);
        if let Some(device) = device {
            self.add_space_pick_device(device, cx);
            self.add_space_goto_location("Projects".into(), (!path.is_empty()).then_some(path), cx);
        }
        cx.notify();
    }

    fn project_open_folder(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let device = self.project_setup.as_ref().and_then(|s| s.device.clone());
        self.project_setup = None;
        if let Some(device) = device {
            self.add_space_pick_device(device, cx);
            self.add_space_goto_location(
                if path.is_some() { "Projects" } else { "Home" }.into(),
                path,
                cx,
            );
        }
    }

    fn project_choose_repository(
        &mut self,
        repository: GithubProjectRepository,
        cx: &mut Context<Self>,
    ) {
        if let Some(setup) = &mut self.project_setup {
            setup
                .url
                .update(cx, |input, cx| input.set_text(repository.https_url, cx));
            let name = repository
                .name_with_owner
                .rsplit('/')
                .next()
                .unwrap_or("project")
                .to_owned();
            setup.name.update(cx, |input, cx| input.set_text(name, cx));
            setup.error = None;
        }
        self.project_enter_clone_confirm(cx);
    }

    fn submit_project_creation(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(setup) = self.project_setup.as_ref().filter(|s| !s.busy) else {
            return;
        };
        let name = setup.name.read(cx).text().trim().to_owned();
        let url = setup.clone_url(cx);
        let name = if name.is_empty() && setup.mode == Mode::Clone {
            clone_name(&url)
        } else {
            name
        };
        let request = StartProjectCreation {
            id: setup.generation.clone(),
            name,
            parent: setup.parent.read(cx).text().trim().into(),
            source: if setup.mode == Mode::Clone {
                ProjectSource::Clone { url }
            } else {
                ProjectSource::Create {
                    template: setup.template,
                    publish: setup.publish,
                }
            },
        };
        let generation = setup.generation.clone();
        let params = self.project_params(serde_json::to_value(request).unwrap(), cx);
        if let Some(setup) = &mut self.project_setup {
            setup.busy = true;
            setup.error = None;
        }
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<ProjectCreation>(START, params)
                .await;
            this.update(cx, |shell, cx| {
                match result {
                    Ok(operation) => {
                        shell.state.update(cx, |state, cx| {
                            let ops = state
                                .project_creations
                                .entry(operation.device_id.clone())
                                .or_default();
                            if !ops.iter().any(|op| op.id == operation.id) {
                                ops.push(operation.clone());
                            }
                            if !state.spaces.iter().any(|s| s.id == operation.id) {
                                state.spaces.push(Space {
                                    id: operation.id.clone(),
                                    device_id: operation.device_id.clone(),
                                    path: operation.destination.clone(),
                                    name: Some(operation.request.name.clone()),
                                    git_detected: true,
                                    git_checked_at: None,
                                    checkout_id: None,
                                    repository_id: None,
                                    remote_repository: None,
                                    created_at: chrono::Utc::now(),
                                });
                            }
                            cx.notify();
                        });
                        if shell
                            .project_setup
                            .as_ref()
                            .is_some_and(|s| s.generation == generation)
                        {
                            shell.close_add_space(cx);
                            shell.land_in_space(operation.id, cx);
                        }
                    }
                    Err(error) => {
                        if let Some(setup) = shell.project_setup_generation(&generation) {
                            setup.busy = false;
                            setup.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn project_action_rows(&self, cx: &App) -> Vec<SetupRow> {
        let Some(setup) = &self.project_setup else {
            return Vec::new();
        };
        let query = setup.query.read(cx).text().to_lowercase();
        let matches = |text: &str| {
            let text = text.to_lowercase();
            query.split_whitespace().all(|q| text.contains(q))
        };
        let mut rows: Vec<SetupRow> = [SetupRow::Open, SetupRow::Clone, SetupRow::Create]
            .into_iter()
            .filter(|row| {
                let (_, title, hint) = row.action();
                matches(&format!("{title} {hint}"))
            })
            .collect();
        let mut spaces: Vec<_> = self
            .state
            .read(cx)
            .spaces
            .iter()
            .filter(|s| Some(s.device_id.as_str()) == setup.device.as_ref().map(|d| d.id.as_str()))
            .filter(|s| matches(&format!("{} {}", s.display_name(), s.path)))
            .collect();
        spaces.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        rows.extend(spaces.into_iter().take(12).map(|s| SetupRow::Project {
            id: s.id.clone(),
            name: s.display_name().to_string(),
            path: s.path.clone(),
        }));
        rows
    }

    fn project_setup_hover(&mut self, active: usize, cx: &mut Context<Self>) {
        if let Some(setup) = &mut self.project_setup
            && (setup.active != active || !setup.navigating)
        {
            setup.active = active;
            setup.navigating = true;
            cx.notify();
        }
    }

    fn project_pick_action(&mut self, row: SetupRow, cx: &mut Context<Self>) {
        match row {
            SetupRow::Project { id, .. } => {
                self.close_add_space(cx);
                self.land_in_space(id, cx);
            }
            SetupRow::Open => self.project_open_folder(None, cx),
            SetupRow::Clone => self.project_mode(Mode::Clone, cx),
            SetupRow::Create => self.project_mode(Mode::Create, cx),
        }
    }

    /// Moves the setup to another computer, keeping the step and the name.
    fn project_pick_host(&mut self, device: Device, cx: &mut Context<Self>) {
        let Some(setup) = &mut self.project_setup else {
            return;
        };
        if setup.device.as_ref().is_some_and(|d| d.id == device.id) {
            return;
        }
        setup.device = Some(device);
        setup.generation = uuid::Uuid::new_v4().to_string();
        setup.github = Loadable::Idle;
        setup.repositories = Loadable::Idle;
        setup.error = None;
        let mode = setup.mode;
        setup.parent.update(cx, |input, cx| input.set_text("", cx));
        self.load_project_parent(cx);
        if mode != Mode::Choose {
            self.load_project_github(1, cx);
        }
        cx.notify();
    }

    /// Steps back: from the clone destination to the repository search, else
    /// to the project actions.
    fn project_back(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = &mut self.project_setup else {
            return;
        };
        if setup.mode == Mode::Clone && setup.clone_confirm {
            setup.clone_confirm = false;
            setup.active = 0;
            setup.navigating = false;
            setup.error = None;
            setup.focus_pending = true;
            cx.notify();
        } else {
            self.project_mode(Mode::Choose, cx);
        }
    }

    /// The input the current step types into.
    fn project_header_input(setup: &ProjectSetup) -> &Entity<ComposerInput> {
        match setup.mode {
            Mode::Create => &setup.name,
            Mode::Clone if setup.clone_confirm => &setup.name,
            _ => &setup.query,
        }
    }

    /// Other computers that are online; the computer choice only appears with them.
    fn project_computers(&self, cx: &App) -> Vec<Device> {
        let state = self.state.read(cx);
        let now = Utc::now();
        let current = self
            .project_setup
            .as_ref()
            .and_then(|s| s.device.as_ref())
            .map(|d| d.id.clone());
        let local = state.local_device_id.clone();
        let others_online = state
            .devices
            .iter()
            .any(|d| Some(&d.id) != local.as_ref() && state.device_online(&d.id, now));
        if !others_online {
            return Vec::new();
        }
        state
            .devices
            .iter()
            .filter(|d| {
                Some(&d.id) == local.as_ref()
                    || Some(&d.id) == current.as_ref()
                    || state.device_online(&d.id, now)
            })
            .cloned()
            .collect()
    }

    /// The selectable rows under the header, in keyboard order.
    fn project_form_rows(&self, cx: &App) -> Vec<FormRow> {
        let Some(setup) = &self.project_setup else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        match setup.mode {
            Mode::Choose => return rows,
            Mode::Clone if !setup.clone_confirm => {
                let query = setup.query.read(cx).text().trim().to_owned();
                if looks_like_git_url(&query) {
                    rows.push(FormRow::Url(query));
                } else {
                    rows.extend(
                        setup
                            .matching_repositories(cx)
                            .into_iter()
                            .map(FormRow::Repo),
                    );
                    if let Some(page) = setup.repositories.ready().and_then(|r| r.next_page) {
                        rows.push(FormRow::More(page));
                    }
                }
                return rows;
            }
            Mode::Create => {
                rows.extend(TEMPLATES.iter().map(|(t, ..)| FormRow::Template(*t)));
                rows.push(FormRow::Publish);
            }
            Mode::Clone => {}
        }
        rows.push(FormRow::Location);
        rows.extend(
            self.project_computers(cx)
                .into_iter()
                .map(FormRow::Computer),
        );
        rows
    }

    fn project_activate_row(&mut self, row: FormRow, cx: &mut Context<Self>) {
        match row {
            FormRow::Template(template) => {
                if let Some(setup) = &mut self.project_setup {
                    setup.template = template;
                }
            }
            FormRow::Publish => {
                let Some(setup) = &mut self.project_setup else {
                    return;
                };
                match &setup.github {
                    Loadable::Ready(s) if s.authenticated => setup.publish = !setup.publish,
                    Loadable::Loading => {}
                    _ => self.load_project_github(1, cx),
                }
            }
            FormRow::Location => self.project_browse_parent(cx),
            FormRow::Computer(device) => self.project_pick_host(device, cx),
            FormRow::Repo(repo) => self.project_choose_repository(repo, cx),
            FormRow::Url(url) => self.project_continue_with_url(url, cx),
            FormRow::More(page) => self.load_project_github(page, cx),
        }
        cx.notify();
    }

    fn project_continue_with_url(&mut self, url: String, cx: &mut Context<Self>) {
        let Some(setup) = &mut self.project_setup else {
            return;
        };
        let name = clone_name(&url);
        setup.url.update(cx, |input, cx| input.set_text(url, cx));
        setup.name.update(cx, |input, cx| input.set_text(name, cx));
        self.project_enter_clone_confirm(cx);
    }

    fn project_enter_clone_confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(setup) = &mut self.project_setup {
            setup.clone_confirm = true;
            setup.active = 0;
            setup.navigating = false;
            setup.error = None;
            setup.focus_pending = true;
            setup
                .name
                .update(cx, |input, cx| input.set_placeholder("Folder name", cx));
        }
        cx.notify();
    }

    /// Enter without a highlighted row: create, clone, or continue to the
    /// clone destination with the pasted URL or the first repository.
    fn project_primary(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = &self.project_setup else {
            return;
        };
        if setup.mode == Mode::Clone && !setup.clone_confirm {
            let first = self
                .project_form_rows(cx)
                .into_iter()
                .find(|row| matches!(row, FormRow::Repo(_) | FormRow::Url(_)));
            if let Some(row) = first {
                self.project_activate_row(row, cx);
            }
            return;
        }
        self.submit_project_creation(cx);
    }

    pub(super) fn render_project_setup(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let setup = self.project_setup.as_mut()?;
        if std::mem::take(&mut setup.focus_pending) {
            let input = Self::project_header_input(setup);
            window.focus(&input.focus_handle(cx), cx);
        }
        let mode = setup.mode;
        let step = match mode {
            Mode::Choose => "choose",
            Mode::Create => "create",
            Mode::Clone if setup.clone_confirm => "clone-confirm",
            Mode::Clone => "clone",
        };
        let focus = setup.focus.clone();
        let card = command_palette::cmd::card("project-setup", &focus, viewport, &theme)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_add_space(cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                this.project_setup_key(event, window, cx)
            }));
        let (header, panel, footer) = if mode == Mode::Choose {
            self.render_project_picker(viewport, cx)
        } else {
            self.render_project_form(viewport, cx)
        };
        // The panel's content slides in from the side the step was reached
        // from; the header and footer stay put.
        let travel = if mode == Mode::Choose { -8.0 } else { 8.0 };
        let panel_body = if crate::motion::reduced_motion(cx) {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    SharedString::from(format!("project-step-{step}")),
                    crate::motion::DIALOG_IN.animation(),
                    move |el, t| el.relative().opacity(t).left(px(travel * (1.0 - t))),
                )
                .into_any_element()
        };
        let card = card
            .child(header)
            .child(command_palette::cmd::panel(&theme).child(panel_body))
            .child(footer);
        Some(command_palette::palette_overlay(viewport, card))
    }

    /// The first step: actions and existing projects.
    fn render_project_picker(
        &mut self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> (Div, Stateful<Div>, Div) {
        let theme = Theme::of(cx).for_popup();
        let empty = || (div(), div().id("project-setup-results"), div());
        let Some(setup) = self.project_setup.as_ref() else {
            return empty();
        };
        let active = setup.active;
        let query = setup.query.clone();
        let scroll = setup.scroll.clone();
        let current_device = setup.device.as_ref().map(|d| d.id.clone());
        let filter = query.read(cx).text().to_string();
        let creations = current_device
            .as_ref()
            .and_then(|id| self.state.read(cx).project_creations.get(id))
            .cloned()
            .unwrap_or_default();
        let mut items: Vec<AnyElement> = Vec::new();
        let mut first_project = true;
        for (ix, entry) in self.project_action_rows(cx).into_iter().enumerate() {
            if ix == 0 && !matches!(entry, SetupRow::Project { .. }) {
                items.push(command_palette::cmd::group_label(&theme, "Start").into_any_element());
            }
            let row = command_palette::cmd::row(&theme, ix == active, ("project-setup-row", ix))
                .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, _, cx| {
                    this.project_setup_hover(ix, cx)
                }));
            let row = match &entry {
                SetupRow::Project { id, name, path } => {
                    if std::mem::take(&mut first_project) {
                        items.push(
                            command_palette::cmd::group_label(&theme, "Projects")
                                .when(ix > 0, |el| el.mt(px(6.0)))
                                .into_any_element(),
                        );
                    }
                    let op = creations.iter().find(|op| &op.destination == path);
                    row.child(command_palette::cmd::glyph(&theme, icons::PROJECT_DEFAULT))
                        .child(div().flex_none().max_w(px(220.0)).truncate().child(
                            popover::search_highlight(name.clone().into(), Some(&filter), &theme),
                        ))
                        .child(command_palette::cmd::detail(&theme, path.clone()))
                        .children(
                            op.and_then(|op| creation_badge(op, &theme, format!("setup-op-{id}"))),
                        )
                }
                action => {
                    let (glyph, title, hint) = action.action();
                    row.child(command_palette::cmd::glyph(&theme, glyph))
                        .child(div().flex_none().child(popover::search_highlight(
                            title.into(),
                            Some(&filter),
                            &theme,
                        )))
                        .child(command_palette::cmd::detail(&theme, hint))
                }
            };
            items.push(
                row.on_click(
                    cx.listener(move |this, _, _, cx| this.project_pick_action(entry.clone(), cx)),
                )
                .into_any_element(),
            );
        }
        if items.is_empty() {
            items.push(command_palette::cmd::empty(&theme, "Nothing matches").into_any_element());
        }
        let header = command_palette::cmd::header(
            &theme,
            command_palette::cmd::search_glyph(&theme),
            query.into_any_element(),
        );
        let panel =
            command_palette::cmd::list("project-setup-results", &scroll, viewport).children(items);
        let footer = command_palette::cmd::footer(&theme)
            .child(
                command_palette::cmd::hints()
                    .child(command_palette::cmd::hint(
                        &theme,
                        &[
                            command_palette::cmd::Key::Up,
                            command_palette::cmd::Key::Down,
                        ],
                        "Navigate",
                    ))
                    .child(command_palette::cmd::hint(
                        &theme,
                        &[command_palette::cmd::Key::Enter],
                        "Open",
                    )),
            )
            .child(command_palette::cmd::hint(
                &theme,
                &[command_palette::cmd::Key::Text("Esc".into())],
                "Close",
            ));
        (header, panel, footer)
    }

    /// Create, and both clone steps: the input names the project or
    /// searches repositories; a summary and options sit in the panel; every
    /// action and shortcut is in the footer.
    fn render_project_form(
        &mut self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> (Div, Stateful<Div>, Div) {
        let theme = Theme::of(cx).for_popup();
        let rows = self.project_form_rows(cx);
        let local_id = self.state.read(cx).local_device_id.clone();
        let Some(setup) = self.project_setup.as_ref() else {
            return (div(), div().id("project-form-results"), div());
        };
        let mode = setup.mode;
        let confirm = mode == Mode::Clone && setup.clone_confirm;
        let searching = mode == Mode::Clone && !confirm;
        let active = setup.active;
        let navigating = setup.navigating;
        let busy = setup.busy;
        let template = setup.template;
        let publish = setup.publish;
        let error = setup.error.clone();
        let scroll = setup.scroll.clone();
        let github = setup.github.clone();
        let github_ok = github.ready().is_some_and(|s| s.authenticated);
        let repos_loading = matches!(setup.repositories, Loadable::Loading);
        let repo_error = setup.repositories.error().map(str::to_owned);
        let input = Self::project_header_input(setup).clone();
        let device = setup.device.clone();
        let url = setup.clone_url(cx);
        let name_text = setup.name.read(cx).text().trim().to_owned();
        let parent_text = setup.parent.read(cx).text().trim().to_owned();
        let destination = (!parent_text.is_empty() && !name_text.is_empty()).then(|| {
            format!(
                "{}/{}",
                parent_text.trim_end_matches(['/', '\\']),
                name_text
            )
        });
        let on_device = device
            .as_ref()
            .filter(|d| local_id.as_deref() != Some(d.id.as_str()))
            .map(|d| format!(" on {}", d.name))
            .unwrap_or_default();
        let has_target = rows
            .iter()
            .any(|row| matches!(row, FormRow::Repo(_) | FormRow::Url(_)));
        let can_submit = !busy
            && if searching {
                has_target
            } else {
                destination.is_some() && (mode != Mode::Clone || !url.is_empty())
            };

        let mut items: Vec<AnyElement> = Vec::new();
        if !searching {
            let (glyph, title, detail) = if confirm {
                (
                    if url.contains("github.com") {
                        icons::SITE_GITHUB
                    } else {
                        icons::GLOBAL
                    },
                    repository_label(&url),
                    match &destination {
                        Some(path) => format!("Clones into {path}{on_device}"),
                        None => "Name the folder to clone into".into(),
                    },
                )
            } else {
                (
                    icons::FOLDER_ADD,
                    if name_text.is_empty() {
                        "New project".to_owned()
                    } else {
                        name_text.clone()
                    },
                    match &destination {
                        Some(path) => format!("Creates {path}{on_device}"),
                        None if parent_text.is_empty() => "Choose a location below".into(),
                        None => format!("Goes in {parent_text}{on_device}"),
                    },
                )
            };
            items.push(
                command_palette::cmd::group_label(
                    &theme,
                    if confirm { "Repository" } else { "Project" },
                )
                .into_any_element(),
            );
            items.push(
                div()
                    .min_h(px(40.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .text_size(crate::typography::text_sm())
                    .child(command_palette::cmd::glyph(&theme, glyph))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_color(theme.text).child(title))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::typography::text_xs())
                                    .text_color(theme.text_muted)
                                    .child(detail),
                            ),
                    )
                    .into_any_element(),
            );
            if let Some(error) = error.clone() {
                items.push(
                    div()
                        .mx(px(8.0))
                        .mb(px(4.0))
                        .flex()
                        .items_start()
                        .gap(px(8.0))
                        .text_size(crate::typography::text_xs())
                        .line_height(px(17.0))
                        .text_color(theme.danger)
                        .child(
                            icon(icons::DANGER_TRIANGLE)
                                .size(px(13.0))
                                .mt(px(2.0))
                                .flex_none()
                                .text_color(theme.danger),
                        )
                        .child(div().flex_1().min_w_0().child(error))
                        .into_any_element(),
                );
            }
        }

        let mut group: Option<&'static str> = None;
        for (ix, entry) in rows.iter().cloned().enumerate() {
            let heading = entry.group();
            if heading.is_some() && heading != group {
                items.push(
                    command_palette::cmd::group_label(&theme, heading.unwrap_or_default())
                        .when(!items.is_empty(), |el| el.mt(px(6.0)))
                        .into_any_element(),
                );
            }
            group = heading;
            let row = command_palette::cmd::row(
                &theme,
                navigating && ix == active,
                ("project-form-row", ix),
            )
            .on_mouse_move(cx.listener(
                move |this, _: &gpui::MouseMoveEvent, _, cx| this.project_setup_hover(ix, cx),
            ));
            let row = match &entry {
                FormRow::Template(choice) => {
                    let (_, label, hint) = TEMPLATES
                        .iter()
                        .copied()
                        .find(|(t, ..)| t == choice)
                        .unwrap_or(TEMPLATES[0]);
                    row.child(command_palette::cmd::glyph(&theme, template_glyph(*choice)))
                        .child(div().flex_none().child(label))
                        .child(command_palette::cmd::detail(&theme, hint))
                        .child(popover::radio_dot(&theme, template == *choice))
                }
                FormRow::Publish => {
                    let detail: SharedString = match &github {
                        _ if github_ok => "Pushes the first commit".into(),
                        Loadable::Loading => "Checking GitHub…".into(),
                        Loadable::Ready(s) if !s.installed => {
                            "Install the GitHub CLI to enable".into()
                        }
                        Loadable::Ready(_) => "Run gh auth login to enable".into(),
                        _ => "GitHub is unavailable here".into(),
                    };
                    let on = publish && github_ok;
                    row.child(command_palette::cmd::glyph(&theme, icons::SITE_GITHUB))
                        .child(
                            div()
                                .flex_none()
                                .text_color(if github_ok {
                                    theme.text
                                } else {
                                    theme.text_muted
                                })
                                .child("Create private GitHub repository"),
                        )
                        .child(command_palette::cmd::detail(&theme, detail))
                        .child(
                            crate::settings::widgets::toggle_switch(&theme, on, "project-publish")
                                .rounded_full()
                                .bg(crate::theme::ink(0.16))
                                .when(!github_ok, |el| el.opacity(0.4)),
                        )
                }
                FormRow::Location => {
                    row.child(command_palette::cmd::glyph(&theme, icons::FOLDER))
                        .child(div().flex_1().min_w_0().truncate().child(
                            if parent_text.is_empty() {
                                "Choose a folder".to_owned()
                            } else {
                                parent_text.clone()
                            },
                        ))
                        .child(command_palette::cmd::trailing(&theme, "Change"))
                }
                FormRow::Computer(d) => {
                    let current = device.as_ref().is_some_and(|c| c.id == d.id);
                    let label = if local_id.as_deref() == Some(d.id.as_str()) {
                        "This computer".to_owned()
                    } else {
                        d.name.clone()
                    };
                    row.child(command_palette::cmd::glyph(
                        &theme,
                        spaces::device_glyph(&d.platform),
                    ))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(popover::radio_dot(&theme, current))
                }
                FormRow::Repo(repo) => {
                    let (owner, repo_name) = repo
                        .name_with_owner
                        .split_once('/')
                        .map(|(o, n)| (o.to_owned(), n.to_owned()))
                        .unwrap_or_else(|| (String::new(), repo.name_with_owner.clone()));
                    row.child(command_palette::cmd::glyph(&theme, icons::SITE_GITHUB))
                        .child(div().flex_none().child(repo_name))
                        .child(command_palette::cmd::detail(&theme, owner))
                        .when(repo.private, |el| {
                            el.child(command_palette::cmd::trailing(&theme, "Private"))
                        })
                }
                FormRow::Url(url) => row
                    .child(command_palette::cmd::glyph(&theme, icons::GLOBAL))
                    .child(div().flex_none().child("Clone from URL"))
                    .child(command_palette::cmd::detail(&theme, url.clone())),
                FormRow::More(_) => row
                    .child(command_palette::cmd::glyph(&theme, icons::REFRESH))
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.text_muted)
                            .child("Load more repositories"),
                    ),
            };
            items.push(
                row.on_click(
                    cx.listener(move |this, _, _, cx| this.project_activate_row(entry.clone(), cx)),
                )
                .into_any_element(),
            );
        }
        if searching && !has_target {
            let note: SharedString = match &github {
                _ if repos_loading => "Loading your repositories…".into(),
                _ if repo_error.is_some() => repo_error.clone().unwrap_or_default().into(),
                Loadable::Loading => "Checking GitHub on this computer…".into(),
                Loadable::Ready(s) if !s.installed => {
                    "Install the GitHub CLI to browse your repositories, or paste a Git URL.".into()
                }
                Loadable::Ready(s) if !s.authenticated => {
                    "Run gh auth login to browse your repositories, or paste a Git URL.".into()
                }
                Loadable::Ready(_) => {
                    "No repositories match. Paste a Git URL to clone anything else.".into()
                }
                _ => "GitHub is unavailable here. Paste a Git URL to clone.".into(),
            };
            items.push(if repos_loading {
                popover::skeleton_rows("project-repos-loading", &theme, 4, cx.entity_id(), cx)
            } else {
                command_palette::cmd::empty(&theme, note).into_any_element()
            });
        }

        let back = div()
            .id("project-form-back")
            .group("project-form-back")
            .size(px(22.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .cursor_pointer()
            .role(gpui::Role::Button)
            .aria_label("Back")
            .hover(|s| s.bg(crate::theme::ink(0.06)))
            .on_click(cx.listener(|this, _, _, cx| this.project_back(cx)))
            .child(
                icon(icons::ARROW_LEFT)
                    .size(px(16.0))
                    .text_color(theme.text_muted)
                    .group_hover("project-form-back", |s| s.text_color(theme.text)),
            )
            .into_any_element();
        let header = command_palette::cmd::header(&theme, back, input.into_any_element());
        let panel =
            command_palette::cmd::list("project-form-results", &scroll, viewport).children(items);

        let highlighted = navigating.then(|| rows.get(active)).flatten();
        let enter_label = match highlighted {
            Some(FormRow::Publish) => "Toggle",
            Some(FormRow::Location) => "Change",
            Some(FormRow::More(_)) => "Load more",
            Some(_) => "Select",
            None if searching => "Continue",
            None if confirm => "Clone",
            None => "Create",
        };
        let (action, keys): (&str, &[command_palette::cmd::Key]) = match (mode, confirm, busy) {
            (_, _, true) => (
                "Starting…",
                &[
                    command_palette::cmd::Key::Mod,
                    command_palette::cmd::Key::Enter,
                ],
            ),
            (Mode::Clone, false, _) => ("Continue", &[command_palette::cmd::Key::Enter]),
            (Mode::Clone, true, _) => (
                "Clone",
                &[
                    command_palette::cmd::Key::Mod,
                    command_palette::cmd::Key::Enter,
                ],
            ),
            _ => (
                "Create",
                &[
                    command_palette::cmd::Key::Mod,
                    command_palette::cmd::Key::Enter,
                ],
            ),
        };
        let footer = command_palette::cmd::footer(&theme)
            .child(
                command_palette::cmd::hints()
                    .child(command_palette::cmd::hint(
                        &theme,
                        &[
                            command_palette::cmd::Key::Up,
                            command_palette::cmd::Key::Down,
                        ],
                        "Navigate",
                    ))
                    .child(command_palette::cmd::hint(
                        &theme,
                        &[command_palette::cmd::Key::Enter],
                        enter_label,
                    ))
                    .child(command_palette::cmd::hint(
                        &theme,
                        &[command_palette::cmd::Key::Text("⌫".into())],
                        "Back",
                    )),
            )
            .child(
                div()
                    .id("project-submit")
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(26.0))
                    .pl(px(8.0))
                    .pr(px(4.0))
                    .mr(px(-4.0))
                    .rounded(px(7.0))
                    .role(gpui::Role::Button)
                    .aria_label(action)
                    .text_color(theme.text)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .when(can_submit, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(crate::theme::ink(0.06)))
                            .active(|s| s.opacity(0.8))
                            .on_click(cx.listener(|this, _, _, cx| this.project_primary(cx)))
                    })
                    .when(!can_submit, |el| el.opacity(0.4))
                    .child(action)
                    .child(command_palette::cmd::keys(&theme, keys)),
            );
        (header, panel, footer)
    }

    fn project_setup_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(setup) = self.project_setup.as_ref() else {
            return;
        };
        let mode = setup.mode;
        let active = setup.active;
        let navigating = setup.navigating;
        let key = match (
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.control,
        ) {
            ("n", true) => "down",
            ("p", true) => "up",
            (key, _) => key,
        };
        if key == "escape" {
            self.close_add_space(cx);
            cx.stop_propagation();
            return;
        }
        if key == "backspace"
            && mode != Mode::Choose
            && Self::project_header_input(setup).read(cx).is_empty()
        {
            self.project_back(cx);
            cx.stop_propagation();
            return;
        }
        if key == "tab" {
            cx.stop_propagation();
            return;
        }
        if mode == Mode::Choose {
            let rows = self.project_action_rows(cx);
            if key == "up" || key == "down" {
                if let Some(setup) = &mut self.project_setup {
                    setup.active = popover::menu_step(
                        Some(active),
                        rows.len(),
                        if key == "up" { -1 } else { 1 },
                    )
                    .unwrap_or(0);
                    setup.scroll.scroll_to_item(setup.active);
                }
                cx.notify();
                cx.stop_propagation();
            } else if key == "enter" {
                if let Some(row) = rows.get(active).cloned() {
                    self.project_pick_action(row, cx);
                }
                cx.stop_propagation();
            }
            return;
        }
        let rows = self.project_form_rows(cx);
        if key == "up" || key == "down" {
            if let Some(setup) = &mut self.project_setup {
                let from = navigating.then_some(active);
                setup.active =
                    popover::menu_step(from, rows.len(), if key == "up" { -1 } else { 1 })
                        .unwrap_or(0);
                setup.navigating = !rows.is_empty();
                setup.scroll.scroll_to_item(setup.active);
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if key == "enter" {
            let submit = event.keystroke.modifiers.platform || event.keystroke.modifiers.control;
            match rows.get(active).cloned() {
                Some(row) if navigating && !submit => self.project_activate_row(row, cx),
                _ => self.project_primary(cx),
            }
            let _ = window;
            cx.stop_propagation();
        }
    }
}

/// A selectable row in the create and clone steps.
#[derive(Clone, Debug)]
enum FormRow {
    Template(ProjectTemplate),
    Publish,
    Location,
    Computer(Device),
    Repo(GithubProjectRepository),
    Url(String),
    More(u32),
}

impl FormRow {
    fn group(&self) -> Option<&'static str> {
        match self {
            FormRow::Template(_) => Some("Template"),
            FormRow::Publish => Some("Options"),
            FormRow::Location => Some("Location"),
            FormRow::Computer(_) => Some("Computer"),
            FormRow::Repo(_) | FormRow::More(_) => Some("Your repositories"),
            FormRow::Url(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SetupRow {
    Open,
    Clone,
    Create,
    Project {
        id: String,
        name: String,
        path: String,
    },
}

impl SetupRow {
    fn action(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            SetupRow::Open => (icons::FOLDER_OPEN, "Open folder", "Add an existing folder"),
            SetupRow::Clone => (
                icons::GIT_BRANCH,
                "Clone repository",
                "From GitHub or any Git URL",
            ),
            SetupRow::Create => (
                icons::FOLDER_ADD,
                "Create project",
                "Start fresh from a template",
            ),
            SetupRow::Project { .. } => (icons::PROJECT_DEFAULT, "", ""),
        }
    }

    #[cfg(test)]
    fn title(&self) -> &str {
        match self {
            SetupRow::Project { name, .. } => name,
            row => row.action().1,
        }
    }
}

const TEMPLATES: [(ProjectTemplate, &str, &str); 3] = [
    (
        ProjectTemplate::Empty,
        "Empty",
        "A README and a Git repository",
    ),
    (
        ProjectTemplate::Nextjs,
        "Next.js",
        "TypeScript, Tailwind and the App Router",
    ),
    (
        ProjectTemplate::Turborepo,
        "Turborepo",
        "A Bun monorepo with apps and packages",
    ),
];

/// A row's trailing creation state: progress while running, the failure otherwise.
fn creation_badge(op: &ProjectCreation, theme: &Theme, key: String) -> Option<AnyElement> {
    let failed = op.error.is_some() || op.publish_error.is_some();
    let running = op.status == ProjectCreationStatus::Running;
    if !running && !failed {
        return None;
    }
    let label = if running {
        match op.percent {
            Some(p) => format!("{} · {p}%", op.stage),
            None => op.stage.clone(),
        }
    } else if op.publish_error.is_some() {
        "Publishing failed".into()
    } else {
        "Failed".into()
    };
    Some(
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(crate::typography::text_xs())
            .text_color(if running {
                theme.text_muted
            } else {
                theme.danger
            })
            .child(if running {
                loaders::ring_spinner(
                    SharedString::from(key),
                    12.0,
                    1.5,
                    crate::theme::hairline(0.12),
                    theme.accent,
                )
            } else {
                icon(icons::DANGER_TRIANGLE)
                    .size(px(13.0))
                    .text_color(theme.danger)
                    .into_any_element()
            })
            .child(div().max_w(px(160.0)).truncate().child(label))
            .into_any_element(),
    )
}

fn template_glyph(template: ProjectTemplate) -> &'static str {
    match template {
        ProjectTemplate::Empty => icons::DOCUMENT,
        ProjectTemplate::Nextjs => icons::GLOBAL,
        ProjectTemplate::Turborepo => icons::WIDGET,
    }
}

fn looks_like_git_url(text: &str) -> bool {
    let text = text.trim();
    !text.contains(char::is_whitespace)
        && (text.contains("://") || (text.starts_with("git@") && text.contains(':')))
}

impl ProjectSetup {
    /// Repositories matching the search, unless the search is a pasted URL.
    fn matching_repositories(&self, cx: &App) -> Vec<GithubProjectRepository> {
        let filter = self.query.read(cx).text().to_lowercase();
        if looks_like_git_url(&filter) {
            return Vec::new();
        }
        self.repositories
            .ready()
            .into_iter()
            .flat_map(|r| &r.repositories)
            .filter(|r| {
                filter
                    .split_whitespace()
                    .all(|q| r.name_with_owner.to_lowercase().contains(q))
            })
            .cloned()
            .collect()
    }

    /// A URL pasted into the search wins over the picked repository.
    fn clone_url(&self, cx: &App) -> String {
        let query = self.query.read(cx).text().trim();
        if looks_like_git_url(query) {
            query.to_owned()
        } else {
            self.url.read(cx).text().trim().to_owned()
        }
    }
}

fn clone_name(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".git")
        .to_owned()
}

/// `owner/name` from a clone URL.
fn repository_label(url: &str) -> String {
    let mut parts = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .take(2)
        .collect::<Vec<_>>();
    parts.reverse();
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn shell(cx: &mut TestAppContext, dir: &std::path::Path) -> Entity<Shell> {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        cx.new(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.local_device_id = Some("local".into());
                state.devices = serde_json::from_value(serde_json::json!([
                    {"id":"local","name":"Laptop","platform":"macos","lastSeenAt":null},
                    {"id":"remote","name":"Build server","platform":"linux","lastSeenAt":null}
                ]))
                .unwrap();
                state
            });
            Shell::new(
                state,
                EngineBootConfig {
                    target: None,
                    data_dir: dir.into(),
                    ipc_port: 0,
                    edge_url: String::new(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zuse_protocol::HarnessId::Mock,
                },
                cx,
            )
        })
    }

    #[gpui::test]
    fn project_picker_starts_with_actions_on_local_host(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(cx, dir.path());
        shell.update(cx, |shell, cx| {
            let start = std::time::Instant::now();
            shell.open_add_space(cx);
            eprintln!(
                "project picker cached initialization: {:?}",
                start.elapsed()
            );
            let setup = shell.project_setup.as_ref().unwrap();
            assert!(setup.mode == Mode::Choose);
            assert_eq!(setup.device.as_ref().unwrap().id, "local");
            assert_eq!(
                shell
                    .project_action_rows(cx)
                    .iter()
                    .map(|row| row.title().to_owned())
                    .collect::<Vec<_>>(),
                ["Open folder", "Clone repository", "Create project"]
            );
            shell.open_add_space_on_device("remote", cx);
            assert_eq!(
                shell
                    .project_setup
                    .as_ref()
                    .unwrap()
                    .device
                    .as_ref()
                    .unwrap()
                    .id,
                "remote"
            );
        });
    }

    #[gpui::test]
    fn choosing_a_folder_reuses_navigation_and_template_defaults_are_explicit(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(cx, dir.path());
        shell.update(cx, |shell, cx| {
            shell.open_add_space(cx);
            shell.project_mode(Mode::Create, cx);
            assert_eq!(
                shell.project_setup.as_ref().unwrap().template,
                ProjectTemplate::Empty
            );
            assert!(!shell.project_setup.as_ref().unwrap().publish);
            shell.project_browse_parent(cx);
            assert!(shell.project_setup.is_none());
            assert!(shell.project_parent_setup.is_some());
            shell.close_add_space(cx);
            assert!(shell.project_setup.as_ref().unwrap().mode == Mode::Create);
            shell.project_open_folder(Some("/projects".into()), cx);
            assert!(shell.project_setup.is_none());
            assert!(shell.add_space.is_some());
            shell.close_add_space(cx);
            assert!(shell.add_space.is_none());
        });
    }

    #[gpui::test]
    fn computers_appear_only_when_another_is_online_and_switching_keeps_the_form(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(cx, dir.path());
        shell.update(cx, |shell, cx| {
            shell.open_add_space(cx);
            shell.project_mode(Mode::Create, cx);
            let name = shell.project_setup.as_ref().unwrap().name.clone();
            name.update(cx, |input, cx| input.set_text("fieldnotes", cx));
            let rows = shell.project_form_rows(cx);
            assert!(!rows.iter().any(|row| matches!(row, FormRow::Computer(_))));
            assert!(rows.iter().any(|row| matches!(row, FormRow::Publish)));
            let remote = shell.state.read(cx).devices[1].clone();
            shell.project_pick_host(remote, cx);
            let setup = shell.project_setup.as_ref().unwrap();
            assert!(setup.mode == Mode::Create);
            assert_eq!(setup.device.as_ref().unwrap().id, "remote");
            assert_eq!(setup.name.read(cx).text(), "fieldnotes");
        });
    }

    #[gpui::test]
    fn pasting_a_url_continues_to_the_destination_and_backspace_returns(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(cx, dir.path());
        shell.update(cx, |shell, cx| {
            shell.open_add_space(cx);
            shell.project_mode(Mode::Clone, cx);
            let query = shell.project_setup.as_ref().unwrap().query.clone();
            query.update(cx, |input, cx| {
                input.set_text("https://github.com/acme/fieldnotes.git", cx)
            });
            let rows = shell.project_form_rows(cx);
            assert!(matches!(rows.as_slice(), [FormRow::Url(_)]));
            shell.project_primary(cx);
            let setup = shell.project_setup.as_ref().unwrap();
            assert!(setup.clone_confirm);
            assert_eq!(setup.name.read(cx).text(), "fieldnotes");
            assert_eq!(
                setup.clone_url(cx),
                "https://github.com/acme/fieldnotes.git"
            );
            assert!(matches!(
                shell.project_form_rows(cx).first(),
                Some(FormRow::Location)
            ));
            shell.project_back(cx);
            let setup = shell.project_setup.as_ref().unwrap();
            assert!(setup.mode == Mode::Clone && !setup.clone_confirm);
            shell.project_back(cx);
            assert!(shell.project_setup.as_ref().unwrap().mode == Mode::Choose);
        });
    }

    #[gpui::test]
    fn repository_selection_populates_clone_fields_and_filter_matches_actions(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(cx, dir.path());
        shell.update(cx, |shell, cx| {
            shell.open_add_space(cx);
            let query = shell.project_setup.as_ref().unwrap().query.clone();
            query.update(cx, |input, cx| input.set_text("clone", cx));
            assert_eq!(shell.project_action_rows(cx).len(), 1);
            shell.project_mode(Mode::Clone, cx);
            shell.project_choose_repository(
                GithubProjectRepository {
                    name_with_owner: "team/My.App".into(),
                    description: None,
                    ssh_url: "git@github.com:team/My.App.git".into(),
                    https_url: "https://github.com/team/My.App.git".into(),
                    private: true,
                },
                cx,
            );
            let setup = shell.project_setup.as_ref().unwrap();
            assert!(setup.clone_confirm);
            assert_eq!(setup.name.read(cx).text(), "My.App");
            assert_eq!(
                setup.url.read(cx).text(),
                "https://github.com/team/My.App.git"
            );
        });
    }
}

#[cfg(feature = "project-palette-fixture")]
impl Shell {
    /// Deterministic project surfaces for the isolated native fixture.
    pub fn fixture_open_project_setup(&mut self, mode: &str, cx: &mut Context<Self>) {
        if mode == "folder" {
            self.open_project_setup(None, cx);
            self.project_open_folder(None, cx);
            cx.notify();
            return;
        }
        self.open_project_setup(None, cx);
        if mode == "create" {
            self.project_mode(Mode::Create, cx);
        }
        if mode.starts_with("clone") {
            self.project_mode(Mode::Clone, cx);
        }
        if let Some(setup) = &mut self.project_setup {
            setup
                .parent
                .update(cx, |input, cx| input.set_text("/home/alex/Developer", cx));
            if mode == "create" {
                setup
                    .name
                    .update(cx, |input, cx| input.set_text("fieldnotes", cx));
                setup.template = ProjectTemplate::Nextjs;
            }
            setup.github = Loadable::Ready(GithubProjectStatus {
                installed: true,
                authenticated: true,
            });
            setup.repositories = Loadable::Ready(GithubProjectRepositories {
                next_page: None,
                repositories: ["fieldnotes", "api", "website"]
                    .into_iter()
                    .map(|name| GithubProjectRepository {
                        name_with_owner: format!("acme/{name}"),
                        description: None,
                        ssh_url: format!("git@github.com:acme/{name}.git"),
                        https_url: format!("https://github.com/acme/{name}.git"),
                        private: true,
                    })
                    .collect(),
            });
        }
        if mode == "clone-confirm"
            && let Some(repo) = self
                .project_setup
                .as_ref()
                .and_then(|s| s.repositories.ready())
                .and_then(|r| r.repositories.first().cloned())
        {
            self.project_choose_repository(repo, cx);
        }
        if mode == "progress" || mode == "failed" {
            let failed = mode == "failed";
            self.close_add_space(cx);
            let operation = ProjectCreation {
                id: "project".into(),
                device_id: "local".into(),
                destination: "/tmp/fieldnotes".into(),
                request: StartProjectCreation {
                    id: "project".into(),
                    name: "fieldnotes".into(),
                    parent: "/tmp".into(),
                    source: ProjectSource::Clone {
                        url: "https://github.com/acme/fieldnotes.git".into(),
                    },
                },
                stage: "Cloning".into(),
                status: if failed {
                    ProjectCreationStatus::Failed
                } else {
                    ProjectCreationStatus::Running
                },
                percent: Some(42),
                files_ready: false,
                error: failed
                    .then(|| "Repository not found. Check the URL and your access.".into()),
                warning: None,
                publish_error: None,
            };
            self.state.update(cx, |state, cx| {
                state
                    .project_creations
                    .insert("local".into(), vec![operation]);
                cx.notify();
            });
            self.land_in_space("project".into(), cx);
        }
        cx.notify();
    }
}
