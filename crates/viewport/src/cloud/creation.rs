//! Project-first creation controls. Opening menus only reads placement metadata.
use super::*;
use crate::creation_style as ui;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
#[derive(Default)]
pub(super) struct State {
    search: Option<Entity<crate::input::TextInput>>,
    active: usize,
    handles: RefCell<BTreeMap<&'static str, FocusHandle>>,
    bounds: RefCell<BTreeMap<&'static str, Rc<Cell<Option<Bounds<Pixels>>>>>>,
}
impl State {
    fn handle(&self, key: &'static str, cx: &mut Context<WorkspaceView>) -> FocusHandle {
        self.handles
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }
    fn bounds(&self, key: &'static str) -> Rc<Cell<Option<Bounds<Pixels>>>> {
        self.bounds.borrow_mut().entry(key).or_default().clone()
    }
}
#[derive(Clone)]
enum Pick {
    Local(Value),
    Repository(usize),
    NoProject,
    AddProject,
    Computer,
    Cloud(usize),
    ChooseCloudProject,
    ConfigureCloud,
    Provider(String),
    Size(String),
    Connection(String),
    AgentSetup,
}
impl WorkspaceView {
    pub(super) fn cloud_computer_catalog(&self) -> Option<Value> {
        let catalogs = self.cloud.provider_catalogs.as_object()?;
        Some(
            self.cloud
                .compute_provider
                .as_deref()
                .and_then(|provider| catalogs.get(provider))
                .cloned()
                .unwrap_or_else(|| json!({"providers":[]})),
        )
    }
    pub(super) fn reconcile_cloud_computer(&mut self) {
        if self.cloud.compute_provider.is_none() {
            self.cloud.compute_provider = self.preferences.cloud_provider.clone();
            self.cloud.compute_size = self
                .cloud
                .compute_provider
                .as_ref()
                .and_then(|provider| self.preferences.cloud_sizes.get(provider))
                .cloned();
        }
        let Some(rows) = self.cloud.capabilities["providers"].as_array() else {
            return;
        };
        let available = |p: &&Value| {
            p["configured"] == true
                && p["managedBaseReady"] == true
                && p["preservesDisk"] == true
                && p["sizes"].as_array().is_some_and(|s| !s.is_empty())
        };
        let provider = rows
            .iter()
            .filter(available)
            .find(|p| p["id"].as_str() == self.cloud.compute_provider.as_deref())
            .or_else(|| rows.iter().filter(available).next());
        self.cloud.compute_provider = provider.and_then(|p| p["id"].as_str()).map(str::to_owned);
        let sizes = provider.and_then(|p| p["sizes"].as_array());
        if !sizes.is_some_and(|rows| {
            rows.iter()
                .any(|s| s["id"].as_str() == self.cloud.compute_size.as_deref())
        }) {
            self.cloud.compute_size = sizes
                .and_then(|rows| rows.first())
                .and_then(|s| s["id"].as_str())
                .map(str::to_owned);
        }
    }
    fn cloud_provider_label(&self, id: &str) -> String {
        if self.cloud.capabilities["stagingCandidate"]["providerId"] == id {
            format!("{id} · Internal test")
        } else {
            id.into()
        }
    }
    fn remember_cloud_computer(&mut self) {
        if let (Some(provider), Some(size)) =
            (&self.cloud.compute_provider, &self.cloud.compute_size)
        {
            self.preferences.cloud_provider = Some(provider.clone());
            self.preferences
                .cloud_sizes
                .insert(provider.clone(), size.clone());
            self.save_preferences();
        }
    }
    pub(super) fn cloud_selected_connection(&self) -> Option<&Value> {
        let agent = self.composer_provider()?["id"].as_str()?;
        let agent = if agent == "claude-code" {
            "claude"
        } else {
            agent
        };
        let mut eligible = self.cloud.connections.iter().filter(|connection| {
            connection["agent"] == agent && connection["state"] == "connected"
        });
        if let Some(selected) = self.cloud.connection.as_deref() {
            // A remembered choice belongs to one agent. Switching models must
            // resolve the new agent independently, but never replace a missing
            // or revoked choice for the same agent.
            let selected = self
                .cloud
                .connections
                .iter()
                .find(|connection| connection["connectionId"].as_str() == Some(selected))?;
            if selected["agent"] == agent {
                return (selected["state"] == "connected").then_some(selected);
            }
        }
        let only = eligible.next()?;
        eligible.next().is_none().then_some(only)
    }

    fn cloud_connection_picker_visible(&self) -> bool {
        if !self.cloud.mode || !self.cloud.authority {
            return false;
        }
        let Some(agent) = self
            .composer_provider()
            .and_then(|provider| provider["id"].as_str())
        else {
            return false;
        };
        let agent = if agent == "claude-code" {
            "claude"
        } else {
            agent
        };
        self.cloud
            .connections
            .iter()
            .filter(|connection| connection["agent"] == agent && connection["state"] == "connected")
            .take(2)
            .count()
            > 1
    }

    pub(crate) fn cloud_creation_repository_identity(&self) -> Option<&str> {
        self.cloud.projects.get(self.cloud.project)?["repositoryIdentity"].as_str()
    }
    fn creation_choices(
        &self,
        kind: &str,
        cx: &App,
    ) -> Vec<(String, &'static str, Pick, bool, Option<&'static str>)> {
        let mut choices = vec![];
        if kind == "computer" {
            if let Some(providers) = self.cloud.capabilities["providers"].as_array() {
                for provider in providers.iter().filter(|p| {
                    p["configured"] == true
                        && p["managedBaseReady"] == true
                        && p["preservesDisk"] == true
                }) {
                    let Some(id) = provider["id"].as_str() else {
                        continue;
                    };
                    choices.push((
                        self.cloud_provider_label(id),
                        "cloud-dither",
                        Pick::Provider(id.into()),
                        self.cloud.compute_provider.as_deref() == Some(id),
                        None,
                    ));
                }
            }
            if let Some(provider) =
                self.cloud.capabilities["providers"]
                    .as_array()
                    .and_then(|rows| {
                        rows.iter()
                            .find(|p| p["id"].as_str() == self.cloud.compute_provider.as_deref())
                    })
            {
                for size in provider["sizes"].as_array().into_iter().flatten() {
                    let Some(id) = size["id"].as_str() else {
                        continue;
                    };
                    choices.push((
                        format!(
                            "{} · {} CPU · {} MiB",
                            size["label"].as_str().unwrap_or(id),
                            size["vcpus"],
                            size["memoryMib"]
                        ),
                        "monitor",
                        Pick::Size(id.into()),
                        self.cloud.compute_size.as_deref() == Some(id),
                        None,
                    ));
                }
            }
            choices.push((
                "Environment and connections…".into(),
                "settings",
                Pick::ConfigureCloud,
                false,
                None,
            ));
        } else if kind == "connection" {
            let agent = self
                .composer_provider()
                .and_then(|provider| provider["id"].as_str());
            let agent = agent.map(|id| if id == "claude-code" { "claude" } else { id });
            for connection in &self.cloud.connections {
                if connection["agent"].as_str() != agent || connection["state"] != "connected" {
                    continue;
                }
                let Some(id) = connection["connectionId"].as_str() else {
                    continue;
                };
                choices.push((
                    format!(
                        "{} · {}",
                        if connection["connectionScope"] == "organization" {
                            "Organization"
                        } else {
                            "Personal"
                        },
                        connection["accountLabel"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| connection["agent"]
                                .as_str()
                                .unwrap_or("Agent account"))
                    ),
                    "key",
                    Pick::Connection(id.to_owned()),
                    self.cloud.connection.as_deref() == Some(id),
                    None,
                ));
            }
            choices.push((
                "Connect coding agent…".into(),
                "plus",
                Pick::AgentSetup,
                false,
                None,
            ));
        } else if kind == "repository" {
            let query = self
                .cloud
                .creation
                .search
                .as_ref()
                .map(|s| s.read(cx).value(cx).to_lowercase())
                .unwrap_or_default();
            for project in self
                .known_projects()
                .into_iter()
                .filter(|_| !self.cloud_controller)
            {
                let name = project["name"].as_str().unwrap_or("Project").to_owned();
                if !format!("{} {}", name, project["path"].as_str().unwrap_or_default())
                    .to_lowercase()
                    .contains(&query)
                {
                    continue;
                }
                let selected = !self.cloud.mode
                    && self
                        .project
                        .as_ref()
                        .is_some_and(|p| p["path"] == project["path"]);
                choices.push((name, "folder", Pick::Local(project), selected, None));
            }
            for (i, p) in self.cloud.projects.iter().enumerate() {
                if !self.cloud_controller
                    && self.spaces.iter().any(|space| {
                        space.remote_repository.as_deref().is_some_and(|identity| {
                            p["repositoryIdentity"]
                                .as_str()
                                .is_some_and(|v| v.eq_ignore_ascii_case(identity))
                        })
                    })
                {
                    continue;
                }
                let name = p["displayName"].as_str().unwrap_or("Repository").to_owned();
                if !format!(
                    "{name} {}",
                    p["repositoryIdentity"].as_str().unwrap_or_default()
                )
                .to_lowercase()
                .contains(&query)
                {
                    continue;
                }
                choices.push((
                    name,
                    "folder",
                    Pick::Repository(i),
                    self.cloud.mode && self.cloud.project == i,
                    None,
                ));
            }
            choices.push((
                "Add new project…".into(),
                "plus",
                Pick::AddProject,
                false,
                None,
            ));
            if !self.cloud_controller {
                choices.insert(
                    0,
                    (
                        "No project".into(),
                        "",
                        Pick::NoProject,
                        !self.cloud.mode && self.project.is_none(),
                        None,
                    ),
                );
            }
        } else {
            if !self.cloud_controller {
                choices.push((
                    "This computer".into(),
                    "monitor",
                    Pick::Computer,
                    !self.cloud.mode,
                    None,
                ));
            }
            let project = if self.cloud.mode {
                self.cloud
                    .projects
                    .get(self.cloud.project)
                    .map(|_| self.cloud.project)
            } else {
                self.project
                    .as_ref()
                    .and_then(|project| self.spaces.iter().find(|s| project["path"] == s.path))
                    .and_then(|space| space.remote_repository.as_ref())
                    .and_then(|identity| {
                        self.cloud.projects.iter().position(|p| {
                            p["repositoryIdentity"]
                                .as_str()
                                .is_some_and(|v| v.eq_ignore_ascii_case(identity))
                        })
                    })
            };
            if let Some(i) = project {
                choices.push((
                    "Cloud computer".into(),
                    "cloud-dither",
                    Pick::Cloud(i),
                    self.cloud.mode,
                    None,
                ));
            } else if !self.cloud.projects.is_empty() {
                choices.push((
                    "Cloud computer…".into(),
                    "cloud-dither",
                    Pick::ChooseCloudProject,
                    false,
                    None,
                ));
            }
            choices.push((
                "Cloud setup…".into(),
                "cloud-dither",
                Pick::ConfigureCloud,
                false,
                None,
            ));
        }
        choices
    }
    fn close_creation_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(kind) = self.cloud.picker.take() {
            window.focus(&self.cloud.creation.handle(kind, cx), cx);
        }
        cx.notify();
    }
    pub(crate) fn open_creation_picker(
        &mut self,
        kind: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_model_change_pending() {
            return;
        }
        if self.cloud.picker == Some(kind) {
            self.close_creation_picker(window, cx);
            return;
        }
        self.cloud.picker = Some(kind);
        self.cloud.creation.active = 0;
        if kind == "repository" {
            if self.cloud.creation.search.is_none() {
                let input =
                    cx.new(|cx| crate::input::TextInput::new("Search projects…", window, cx));
                cx.observe(&input, |this, _, cx| {
                    this.cloud.creation.active = 0;
                    cx.notify();
                })
                .detach();
                self.cloud.creation.search = Some(input);
            }
            let search = self.cloud.creation.search.as_ref().unwrap();
            search.update(cx, |s, cx| s.set_value(String::new(), cx));
            window.focus(&search.focus_handle(cx), cx);
            self.refresh_cloud();
        } else if kind == "branch" {
            window.focus(&self.cloud_branch.focus_handle(cx), cx);
        } else {
            self.refresh_cloud();
            window.focus(&self.cloud.creation.handle("menu", cx), cx);
        }
        cx.notify();
    }
    fn pick_creation(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud_controller
            && matches!(pick, Pick::Local(_) | Pick::NoProject | Pick::Computer)
        {
            self.close_creation_picker(window, cx);
            return;
        }
        match pick {
            Pick::Local(project) => {
                self.set_creation_destination(false, cx);
                self.project = Some(project);
            }
            Pick::Repository(i) | Pick::Cloud(i) => {
                if self.cloud.project != i || !self.cloud.mode {
                    self.cloud_branch
                        .update(cx, |v, cx| v.set_value(String::new(), cx));
                }
                self.cloud.project = i;
                self.cloud.project_id = self
                    .cloud
                    .projects
                    .get(i)
                    .and_then(|row| row["projectId"].as_str())
                    .map(str::to_owned);
                self.set_creation_destination(true, cx);
            }
            Pick::ChooseCloudProject => {
                self.cloud.project = usize::MAX;
                self.cloud.project_id = None;
                self.set_creation_destination(true, cx);
                self.close_creation_picker(window, cx);
                self.open_creation_picker("repository", window, cx);
                return;
            }
            Pick::NoProject => {
                self.set_creation_destination(false, cx);
                self.project = None;
            }
            Pick::Computer => self.set_creation_destination(false, cx),
            Pick::AddProject => self.open_project_setup(cx),
            Pick::ConfigureCloud => {
                self.settings = true;
                self.select_settings_section(crate::settings_view::Section::Cloud, cx);
            }
            Pick::Provider(id) => {
                self.cloud.compute_size = self.preferences.cloud_sizes.get(&id).cloned();
                self.cloud.compute_provider = Some(id);
                self.reconcile_cloud_computer();
                if let Some(catalog) = self.cloud_computer_catalog() {
                    self.receive_cloud_models(catalog, cx);
                }
                self.cloud.connection = None;
                self.remember_cloud_computer();
            }
            Pick::Size(id) => {
                self.cloud.compute_size = Some(id);
                self.remember_cloud_computer();
            }
            Pick::Connection(id) => {
                self.cloud.connection = Some(id);
            }
            Pick::AgentSetup => {
                self.settings = true;
                self.select_settings_section(crate::settings_view::Section::Cloud, cx);
            }
        }
        self.close_creation_picker(window, cx);
    }
    fn creation_menu(&self, kind: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.cloud.creation.handle("menu", cx);
        let trigger = self.cloud.creation.bounds(kind);
        let mut menu = ui::menu(256.)
            .id("composer-destination-menu")
            .role(Role::Menu)
            .track_focus(&focus)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                if !trigger.get().is_some_and(|b| b.contains(&e.position)) {
                    this.close_creation_picker(window, cx);
                }
            }))
            .capture_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                let key = e.keystroke.key.as_str();
                if key == "escape" {
                    this.close_creation_picker(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if kind == "branch" {
                    if key == "enter" {
                        this.close_creation_picker(window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                let choices = this.creation_choices(kind, cx);
                let count = choices.len();
                match key {
                    "up" => {
                        this.cloud.creation.active =
                            (this.cloud.creation.active + count - 1) % count
                    }
                    "down" => this.cloud.creation.active = (this.cloud.creation.active + 1) % count,
                    "home" => this.cloud.creation.active = 0,
                    "end" => this.cloud.creation.active = count - 1,
                    "enter" | "space" if key == "enter" || kind != "repository" => {
                        if let Some(choice) = choices.get(this.cloud.creation.active) {
                            this.pick_creation(choice.2.clone(), window, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }));
        if kind == "branch" {
            menu = menu
                .child(ui::meta("Base branch").px(px(8.)).py(px(6.)))
                .child(
                    crate::settings_style::field()
                        .px(px(8.))
                        .child(self.cloud_branch.clone()),
                )
                .child(
                    ui::option("cloud-config-done", "Done", "check", false).on_click(
                        cx.listener(|this, _, window, cx| this.close_creation_picker(window, cx)),
                    ),
                );
        } else {
            if kind == "repository" {
                if let Some(search) = &self.cloud.creation.search {
                    menu = menu.child(
                        div().px(px(4.)).pb(px(6.)).child(
                            crate::settings_style::field()
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(icon("magnifer").size(px(14.)).text_color(rgb(MUTED)))
                                .child(div().flex_1().min_w_0().child(search.clone())),
                        ),
                    );
                }
            }
            let mut list = column()
                .id("creation-project-choices")
                .max_h(px(if kind == "repository" { 192. } else { 288. }))
                .overflow_y_scroll();
            let mut footer = None;
            for (i, (label, glyph, pick, selected, detail)) in
                self.creation_choices(kind, cx).into_iter().enumerate()
            {
                if kind == "environment" && i > 0 {
                    list = list.child(ui::separator());
                }
                let option = ui::option(("creation-option", i), label, glyph, selected)
                    .when(kind == "repository", |v| v.h(px(32.)))
                    .when(self.cloud.creation.active == i, |v| {
                        v.bg(crate::settings_style::frame_bg().opacity(0.6))
                    })
                    .when_some(detail, |v, label| v.child(ui::meta(label)))
                    .when(
                        !matches!(
                            pick,
                            Pick::AddProject | Pick::ConfigureCloud | Pick::AgentSetup
                        ),
                        |v| v.child(ui::selection(selected)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_creation(pick.clone(), window, cx)
                    }));
                if kind == "repository" && i + 1 == self.creation_choices(kind, cx).len() {
                    footer = Some(option);
                } else {
                    list = list.child(option);
                }
            }
            menu = menu
                .child(list)
                .when_some(footer, |v, footer| v.child(ui::separator()).child(footer));
        }
        menu.into_any_element()
    }
    fn creation_chip(
        &self,
        kind: &'static str,
        label: String,
        glyph: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let bounds = self.cloud.creation.bounds(kind);
        let measure = bounds.clone();
        let compact = matches!(
            kind,
            "repository" | "environment" | "connection" | "computer"
        );
        let mut view = div()
            .relative()
            .flex_none()
            .when(compact, |view| view.min_w(px(60.)).flex_shrink(1.))
            .child(
                ui::chip(SharedString::from(format!("composer-{kind}")), label, glyph)
                    .when(compact, |view| view.max_w_full().flex_shrink(1.))
                    .relative()
                    .track_focus(&self.cloud.creation.handle(kind, cx).tab_stop(true))
                    .aria_expanded(self.cloud.picker == Some(kind))
                    .child(
                        canvas(move |b, _, _| measure.set(Some(b)), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_creation_picker(kind, window, cx)
                    })),
            );
        if self.cloud.picker == Some(kind) {
            view = view.child(
                div().absolute().bottom_full().left_0().child(deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .offset(point(px(0.), px(-4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(self.creation_menu(kind, cx)),
                )),
            );
        }
        view
    }
    pub(crate) fn creation_project_name(&self) -> Option<String> {
        if self.cloud.mode {
            self.cloud
                .projects
                .get(self.cloud.project)
                .and_then(|p| p["displayName"].as_str())
                .map(str::to_owned)
        } else {
            self.project.as_ref().map(|project| {
                project["name"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.project_name())
            })
        }
    }
    pub(crate) fn cloud_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.cloud.projects.get(self.cloud.project);
        let name = if self.cloud.mode {
            project
                .and_then(|p| p["displayName"].as_str())
                .unwrap_or("Select project")
                .to_owned()
        } else {
            self.project
                .as_ref()
                .and_then(|p| p["name"].as_str())
                .unwrap_or("No project")
                .to_owned()
        };
        row()
            .relative()
            .w(relative(14. / 15.))
            .self_center()
            .min_h(px(40.))
            .px(px(8.))
            .py(px(6.))
            .gap(px(6.))
            .bg(crate::palette::composer_toolbar())
            .when(!self.has_composer_notices(), |v| v.rounded_t(px(19.2)))
            .child(self.creation_chip("repository", name, "folder", cx))
            .child(self.creation_chip(
                "environment",
                if self.cloud.mode {
                    "Cloud computer".into()
                } else {
                    "This computer".into()
                },
                if self.cloud.mode {
                    "cloud-dither"
                } else {
                    "monitor"
                },
                cx,
            ))
            .when(
                self.cloud.mode && self.cloud.compute_provider.is_some(),
                |view| {
                    view.child(self.creation_chip(
                        "computer",
                        format!(
                            "{} · {}",
                            self.cloud_provider_label(
                                self.cloud.compute_provider.as_deref().unwrap_or("Computer")
                            ),
                            self.cloud.compute_size.as_deref().unwrap_or("default")
                        ),
                        "monitor",
                        cx,
                    ))
                },
            )
            .when(self.cloud_connection_picker_visible(), |view| {
                view.child(
                    self.creation_chip(
                        "connection",
                        self.cloud_selected_connection()
                            .and_then(|connection| connection["accountLabel"].as_str())
                            .filter(|label| !label.is_empty())
                            .unwrap_or("Agent account")
                            .to_owned(),
                        "key",
                        cx,
                    ),
                )
            })
            .when_some(self.creation_workspace_control(cx), |view, control| {
                view.child(control)
            })
            .children(self.import_chat_control(cx))
            .map(|view| {
                view.child(div().flex_1())
                    .children(self.creation_source_control(cx))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod destination_tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn hosted_controller_keeps_cloud_destination_across_session_changes(cx: &mut TestAppContext) {
        cx.update(initialize);
        for hosted in [false, true] {
            let (requests, mut sent) = futures::channel::mpsc::unbounded();
            let (_events, events) = futures::channel::mpsc::unbounded();
            let window = cx.add_window(|window, cx| {
                WorkspaceView::new(Transport { requests, events }, window, cx)
            });
            window.update(cx, |view, window, cx| {
                view.verify_engine(&json!({"deviceId":"00000000-0000-4000-8000-000000000001", "capabilities": if hosted { vec!["cloud-controller-v1"] } else { vec![] }}), cx);
                assert_eq!(view.cloud_controller, hosted);
                assert_eq!(view.cloud.mode, hosted);
                let local_choice = |kind| view.creation_choices(kind, cx).iter().any(|row| matches!(row.2, Pick::Local(_) | Pick::Computer | Pick::NoProject));
                assert_eq!(local_choice("repository"), !hosted);
                assert_eq!(local_choice("environment"), !hosted);
                if hosted {
                    assert!(!view.has_cloud_creation_notice());
                    view.cloud.account = Some("signed-in-actor".into());
                    assert!(view.has_cloud_creation_notice());
                    view.cloud.account = None;
                    view.pick_creation(Pick::Computer, window, cx);
                    assert!(view.cloud.mode);
                    view.pick_creation(Pick::AddProject, window, cx);
                    assert!(view.settings);
                    assert!(!view.project_picker);
                    view.project_input.update(cx, |input, cx| input.set_value("/private/local".into(), cx));
                    view.add_project(cx);
                    assert!(!view.project_picker);
                    assert!(view.settings_section == crate::settings_view::Section::General);
                    view.receive_connection(&json!({"connection":"authRequired","hostedSessionRevoked":true}), cx);
                    assert!(view.cloud.mode);
                    view.set_creation_destination(false, cx);
                    assert!(view.cloud.mode);
                    assert_eq!(view.project_name(), "Cloud project");
                }
            }).unwrap();
            while let Ok(frame) = sent.try_recv() {
                assert_ne!(frame["method"], methods::CLOUD_CREATE);
                assert_ne!(frame["method"], methods::MUTATE);
                assert_ne!(frame["method"], methods::ADD_REPO);
            }
        }
    }
    #[gpui::test]
    fn cloud_computer_without_a_local_project_requires_explicit_repository_selection(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, window, cx| {
                view.project = None;
                view.spaces.clear();
                view.cloud.mode = false;
                view.cloud.projects = vec![json!({"projectId":"repo","displayName":"Repository"})];
                assert!(
                    view.creation_choices("environment", cx)
                        .iter()
                        .any(|row| matches!(row.2, Pick::ChooseCloudProject))
                );
                view.pick_creation(Pick::ChooseCloudProject, window, cx);
                assert!(view.cloud.mode);
                assert_eq!(view.cloud.picker, Some("repository"));
                assert_eq!(view.cloud.project, usize::MAX);
                assert!(view.cloud.project_id.is_none());
                assert!(view.cloud.create.is_none());
            })
            .unwrap();
        while let Ok(frame) = sent.try_recv() {
            assert_ne!(frame["method"], methods::CLOUD_CREATE);
            assert_ne!(frame["method"], methods::MUTATE);
        }
    }
    #[gpui::test]
    fn selected_model_automatically_uses_its_sole_account_without_a_picker(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, _sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, _, cx| {
                view.selected = None;
                view.cloud.mode = true;
                view.cloud.authority = true;
                view.cloud_models.providers = vec![
                    json!({"id":"codex"}),
                    json!({"id":"claude-code"}),
                    json!({"id":"grok"}),
                ];
                view.cloud.connections = vec![
                    json!({"connectionId":"codex-account","agent":"codex","state":"connected"}),
                    json!({"connectionId":"claude-account","agent":"claude","state":"connected"}),
                    json!({"connectionId":"grok-account","agent":"grok","state":"connected"}),
                ];
                view.cloud.connection = Some("codex-account".into());
                for (index, account) in [
                    (0, "codex-account"),
                    (1, "claude-account"),
                    (2, "grok-account"),
                ] {
                    view.provider_index = index;
                    assert_eq!(
                        view.cloud_selected_connection().unwrap()["connectionId"],
                        account
                    );
                    assert!(!view.cloud_connection_picker_visible());
                    view.cloud.creation.bounds.borrow_mut().clear();
                    let _toolbar = view.cloud_toolbar(cx);
                    assert!(
                        !view
                            .cloud
                            .creation
                            .bounds
                            .borrow()
                            .contains_key("connection"),
                        "The actual toolbar must omit a redundant account chip"
                    );
                }
                view.cloud.connections[0]["state"] = json!("revoked");
                view.provider_index = 1;
                assert_eq!(
                    view.cloud_selected_connection().unwrap()["connectionId"],
                    "claude-account",
                    "An unrelated revoked agent must not block this model"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn same_agent_ambiguous_accounts_require_choice_and_revocation_never_falls_back(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, _sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, _, cx| {
                view.selected = None;
                view.cloud.mode = true;
                view.cloud.authority = true;
                view.cloud_models.providers = vec![json!({"id":"codex"})];
                view.provider_index = 0;
                view.cloud.connections = vec![
                    json!({"connectionId":"first","agent":"codex","state":"connected"}),
                    json!({"connectionId":"second","agent":"codex","state":"connected"}),
                ];
                view.cloud.connection = None;
                assert!(view.cloud_selected_connection().is_none());
                assert!(view.cloud_connection_picker_visible());
                view.cloud.creation.bounds.borrow_mut().clear();
                let _toolbar = view.cloud_toolbar(cx);
                assert!(
                    view.cloud
                        .creation
                        .bounds
                        .borrow()
                        .contains_key("connection"),
                    "The actual toolbar retains choice for two eligible accounts"
                );
                view.cloud.connection = Some("second".into());
                assert_eq!(
                    view.cloud_selected_connection().unwrap()["connectionId"],
                    "second"
                );
                view.cloud.connections[1]["state"] = json!("revoked");
                assert!(
                    view.cloud_selected_connection().is_none(),
                    "Never replace the selected revoked account with the other one"
                );
                assert!(!view.cloud_connection_picker_visible());
                view.cloud.connections.pop();
                assert!(
                    view.cloud_selected_connection().is_none(),
                    "A missing pinned account is not an automatic fallback"
                );
                view.cloud.connection = None;
                assert_eq!(
                    view.cloud_selected_connection().unwrap()["connectionId"],
                    "first"
                );
            })
            .unwrap();
    }
}
