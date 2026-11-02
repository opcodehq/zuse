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
    AddComputer,
}
impl WorkspaceView {
    pub(crate) fn cloud_creation_repository_identity(&self) -> Option<&str> {
        self.cloud.projects.get(self.cloud.project)?["repositoryIdentity"].as_str()
    }
    fn creation_choices(
        &self,
        kind: &str,
        cx: &App,
    ) -> Vec<(String, &'static str, Pick, bool, Option<&'static str>)> {
        let mut choices = vec![];
        if kind == "repository" {
            let query = self
                .cloud
                .creation
                .search
                .as_ref()
                .map(|s| s.read(cx).value(cx).to_lowercase())
                .unwrap_or_default();
            for project in self.known_projects() {
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
                if self.spaces.iter().any(|space| {
                    space.remote_repository.as_deref().is_some_and(|identity| {
                        p["repositoryIdentity"]
                            .as_str()
                            .is_some_and(|v| v.eq_ignore_ascii_case(identity))
                    })
                }) {
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
        } else {
            choices.push((
                "This computer".into(),
                "monitor",
                Pick::Computer,
                !self.cloud.mode,
                None,
            ));
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
                let p = &self.cloud.projects[i];
                let ready = p["prepared"] == true && p["state"] == "ready";
                choices.push((
                    "Cloud sandbox · boxd".into(),
                    "cloud-dither",
                    Pick::Cloud(i),
                    self.cloud.mode,
                    Some(if ready { "Beta" } else { "Set up cloud" }),
                ));
            }
            choices.push((
                "Add computer…".into(),
                "monitor",
                Pick::AddComputer,
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
        match pick {
            Pick::Local(project) => {
                self.set_creation_destination(false, cx);
                self.project = Some(project);
            }
            Pick::Repository(i) | Pick::Cloud(i) => {
                let p = &self.cloud.projects[i];
                if p["prepared"] != true || p["state"] != "ready" {
                    self.close_creation_picker(window, cx);
                    self.settings = true;
                    self.select_settings_section(crate::settings_view::Section::Cloud, cx);
                    return;
                }
                if self.cloud.project != i || !self.cloud.mode {
                    self.cloud_branch
                        .update(cx, |v, cx| v.set_value(String::new(), cx));
                }
                self.cloud.project = i;
                self.set_creation_destination(true, cx);
            }
            Pick::NoProject => {
                self.set_creation_destination(false, cx);
                self.project = None;
            }
            Pick::Computer => self.set_creation_destination(false, cx),
            Pick::AddProject => self.project_picker = true,
            Pick::AddComputer => {
                self.settings = true;
                self.select_settings_section(crate::settings_view::Section::Devices, cx);
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
                    .when(!matches!(pick, Pick::AddProject | Pick::AddComputer), |v| {
                        v.child(ui::selection(selected))
                    })
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
        let compact = matches!(kind, "repository" | "environment");
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
                    "Cloud sandbox · boxd".into()
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
