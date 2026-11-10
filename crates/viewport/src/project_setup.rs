//! New project in the browser: open a folder on the host, clone a repository,
//! or start from a template. The host does the work and reports its progress.
use super::*;
use crate::assets::icon;
use crate::shell::{BORDER, FAINT, MUTED, RAISED, TEXT, column, control, icon_button, label, row};
use zuse_protocol::project_creation::{
    self as creation, ProjectSource, ProjectTemplate, StartProjectCreation,
};

const NOT_A_REPOSITORY: &str = "Not a git repository: ";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SetupMode {
    Choose,
    Browse,
    Clone,
    Create,
}

pub(super) struct ProjectSetup {
    mode: SetupMode,
    url: Entity<input::TextInput>,
    name: Entity<input::TextInput>,
    parent: Entity<input::TextInput>,
    template: ProjectTemplate,
    listing: Value,
    listing_read: Option<u64>,
    parent_read: Option<u64>,
    start: Option<u64>,
    watch: Option<u64>,
    open_space: Option<(u64, String)>,
    operation: Option<String>,
    stage: Option<String>,
}

impl ProjectSetup {
    pub(super) fn new(window: &mut Window, cx: &mut Context<WorkspaceView>) -> Self {
        Self {
            mode: SetupMode::Choose,
            url: cx
                .new(|cx| input::TextInput::new("https://github.com/owner/repo.git", window, cx)),
            name: cx.new(|cx| input::TextInput::new("my-project", window, cx)),
            parent: cx.new(|cx| input::TextInput::new("Folder on this computer", window, cx)),
            template: ProjectTemplate::Empty,
            listing: Value::Null,
            listing_read: None,
            parent_read: None,
            start: None,
            watch: None,
            open_space: None,
            operation: None,
            stage: None,
        }
    }
    fn busy(&self) -> bool {
        self.start.is_some() || self.operation.is_some() || self.open_space.is_some()
    }
}

fn clone_name(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn parent_of(path: &str) -> Option<String> {
    let path = path.trim_end_matches('/');
    match path.rsplit_once('/') {
        Some(("", name)) if !name.is_empty() => Some("/".to_owned()),
        Some((parent, _)) if !parent.is_empty() => Some(parent.to_owned()),
        _ => None,
    }
}

fn folder_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

impl WorkspaceView {
    pub(super) fn open_project_setup(&mut self, cx: &mut Context<Self>) {
        if !self.project_setup.busy() {
            self.project_setup.mode = SetupMode::Choose;
            self.project_error = None;
        }
        self.project_picker = true;
        cx.notify();
    }

    fn close_project_setup(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.project_setup.watch.take() {
            self.cancel(id);
        }
        self.project_setup.operation = None;
        self.project_setup.stage = None;
        self.project_setup.start = None;
        self.project_setup.open_space = None;
        self.project_setup.mode = SetupMode::Choose;
        self.project_error = None;
        self.project_picker = false;
        cx.notify();
    }

    fn set_project_mode(&mut self, mode: SetupMode, cx: &mut Context<Self>) {
        self.project_setup.mode = mode;
        self.project_error = None;
        match mode {
            SetupMode::Browse => self.browse_project_folders(None, cx),
            SetupMode::Clone | SetupMode::Create => {
                if self.project_setup.parent.read(cx).value(cx).is_empty() {
                    self.project_setup.parent_read =
                        Some(self.call(creation::DEFAULT_PARENT, json!({})));
                }
            }
            SetupMode::Choose => {}
        }
        cx.notify();
    }

    fn browse_project_folders(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if !self.require_connection(cx) {
            return;
        }
        if let Some(id) = self.project_setup.listing_read.take() {
            self.cancel(id);
        }
        self.project_error = None;
        self.project_setup.listing_read =
            Some(self.call(methods::LIST_FOLDERS, json!({ "path": path })));
        cx.notify();
    }

    /// Enter in the path box moves there while browsing.
    pub(super) fn submit_project_path(&mut self, cx: &mut Context<Self>) {
        if self.project_setup.mode == SetupMode::Browse {
            let path = self.project_input.read(cx).value(cx).trim().to_owned();
            if !path.is_empty() {
                self.browse_project_folders(Some(path), cx);
            }
        } else {
            self.add_project(cx);
        }
    }

    fn open_project_folder(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.require_connection(cx) {
            return;
        }
        self.project_error = None;
        self.call(methods::ADD_REPO, json!({ "path": path }));
        cx.notify();
    }

    /// A folder without Git is still a project, as it is on desktop.
    pub(super) fn add_plain_project_folder(&mut self, error: &str, cx: &mut Context<Self>) -> bool {
        let Some(path) = error.strip_prefix(NOT_A_REPOSITORY).map(str::to_owned) else {
            return false;
        };
        let Some(device) = self.device_id.clone() else {
            return false;
        };
        let id = self.call(
            methods::MUTATE,
            json!({
                "op":"createSpace",
                "spaceId":uuid::Uuid::new_v4().to_string(),
                "deviceId":device,
                "path":path,
                "gitDetected":false,
            }),
        );
        self.project_setup.open_space = Some((id, path));
        cx.notify();
        true
    }

    fn submit_project_creation(&mut self, cx: &mut Context<Self>) {
        if self.project_setup.busy() || !self.require_connection(cx) {
            return;
        }
        let setup = &self.project_setup;
        let url = setup.url.read(cx).value(cx).trim().to_owned();
        let typed = setup.name.read(cx).value(cx).trim().to_owned();
        let parent = setup.parent.read(cx).value(cx).trim().to_owned();
        let (name, source) = if setup.mode == SetupMode::Clone {
            let name = if typed.is_empty() {
                clone_name(&url)
            } else {
                typed
            };
            (name, ProjectSource::Clone { url: url.clone() })
        } else {
            (
                typed,
                ProjectSource::Create {
                    template: setup.template,
                    publish: false,
                },
            )
        };
        let missing = if setup.mode == SetupMode::Clone && url.is_empty() {
            Some("Enter a repository URL.")
        } else if name.is_empty() {
            Some("Enter a project name.")
        } else if parent.is_empty() {
            Some("Choose a folder for the project.")
        } else {
            None
        };
        if let Some(message) = missing {
            self.project_error = Some(message.into());
            cx.notify();
            return;
        }
        let request = StartProjectCreation {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            parent,
            source,
        };
        self.project_error = None;
        self.project_setup.stage = Some("Starting…".into());
        self.project_setup.start = Some(self.call(
            creation::START,
            serde_json::to_value(request).unwrap_or_default(),
        ));
        if self.project_setup.watch.is_none() {
            self.project_setup.watch = Some(self.call(creation::WATCH, json!({})));
        }
        cx.notify();
    }

    fn apply_project_creation(&mut self, operation: &Value, cx: &mut Context<Self>) {
        let status = operation["status"].as_str().unwrap_or_default();
        if status == "ready" || operation["filesReady"] == true {
            self.project = Some(json!({
                "path":operation["destination"],
                "name":operation["request"]["name"],
            }));
            self.call(methods::LIST_REPOS, json!({}));
            self.close_project_setup(cx);
        } else if status == "running" {
            let stage = operation["stage"].as_str().unwrap_or("Working");
            self.project_setup.stage = Some(match operation["percent"].as_u64() {
                Some(percent) => format!("{stage} · {percent}%"),
                None => stage.to_owned(),
            });
        } else {
            self.project_setup.operation = None;
            self.project_setup.stage = None;
            self.project_error = Some(
                operation["error"]
                    .as_str()
                    .unwrap_or("The project could not be created.")
                    .to_owned(),
            );
        }
    }

    pub(super) fn receive_project_setup(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let error = frame["err"].as_str().map(str::to_owned);
        if self.project_setup.listing_read == Some(id) {
            self.project_setup.listing_read = None;
            self.pending.remove(&id);
            if let Some(listing) = frame.get("ok") {
                let path = listing["path"].as_str().unwrap_or_default().to_owned();
                self.project_input
                    .update(cx, |input, cx| input.set_value(path, cx));
                self.project_setup.listing = listing.clone();
            }
            self.project_error = error;
        } else if self.project_setup.parent_read == Some(id) {
            self.project_setup.parent_read = None;
            self.pending.remove(&id);
            if let Some(parent) = frame["ok"].as_str().map(str::to_owned) {
                self.project_setup
                    .parent
                    .update(cx, |input, cx| input.set_value(parent, cx));
            }
        } else if self.project_setup.start == Some(id) {
            self.project_setup.start = None;
            self.pending.remove(&id);
            if let Some(operation) = frame.get("ok").cloned() {
                self.project_setup.operation = operation["id"].as_str().map(str::to_owned);
                self.apply_project_creation(&operation, cx);
            } else {
                self.project_setup.stage = None;
                self.project_error = error;
            }
        } else if self.project_setup.watch == Some(id) {
            let current = self.project_setup.operation.clone();
            let operation = frame["item"].as_array().and_then(|operations| {
                operations
                    .iter()
                    .find(|operation| operation["id"].as_str() == current.as_deref())
                    .cloned()
            });
            if let Some(operation) = operation {
                self.apply_project_creation(&operation, cx);
            }
            if frame.get("err").is_some() || frame["done"] == true {
                self.project_setup.watch = None;
                self.pending.remove(&id);
            }
        } else if self
            .project_setup
            .open_space
            .as_ref()
            .is_some_and(|(request, _)| *request == id)
        {
            let (_, path) = self.project_setup.open_space.take().unwrap_or_default();
            self.pending.remove(&id);
            if frame.get("ok").is_some() {
                self.project = Some(json!({"path":path,"name":folder_name(&path)}));
                self.close_project_setup(cx);
            } else {
                self.project_error = error;
            }
        } else {
            return false;
        }
        cx.notify();
        true
    }

    fn project_action(
        &self,
        id: &'static str,
        glyph: &'static str,
        title: &'static str,
        subtitle: &'static str,
        mode: SetupMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        row()
            .id(id)
            .role(Role::Button)
            .aria_label(title)
            .cursor_pointer()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_lg()
            .hover(|style| style.bg(rgb(RAISED)))
            .child(icon(glyph).size(px(16.)).text_color(rgb(MUTED)))
            .child(
                column()
                    .child(label(title).text_size(px(13.)).text_color(rgb(TEXT)))
                    .child(label(subtitle).text_size(px(11.)).text_color(rgb(FAINT))),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_project_mode(mode, cx)))
    }

    fn project_field(
        &self,
        title: &'static str,
        input: &Entity<input::TextInput>,
    ) -> impl IntoElement {
        column()
            .gap_1()
            .child(label(title).text_size(px(11.)).text_color(rgb(MUTED)))
            .child(
                div()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .child(input.clone()),
            )
    }

    fn project_choose_view(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut children = vec![
            self.project_action(
                "project-open-folder",
                "folder",
                "Open folder",
                "Add an existing folder",
                SetupMode::Browse,
                cx,
            )
            .into_any_element(),
            self.project_action(
                "project-clone",
                "git-branch",
                "Clone repository",
                "From GitHub or any Git URL",
                SetupMode::Clone,
                cx,
            )
            .into_any_element(),
            self.project_action(
                "project-create",
                "plus",
                "Create project",
                "Start fresh from a template",
                SetupMode::Create,
                cx,
            )
            .into_any_element(),
        ];
        let projects = self.known_projects();
        if !projects.is_empty() {
            children.push(
                label("Recent")
                    .mt_3()
                    .px_3()
                    .text_size(px(11.))
                    .text_color(rgb(FAINT))
                    .into_any_element(),
            );
        }
        children.extend(projects.into_iter().enumerate().map(|(index, project)| {
            let name = project["name"].as_str().unwrap_or("Project").to_owned();
            let path = project["path"].as_str().unwrap_or_default().to_owned();
            row()
                .id(("project", index))
                .role(Role::Button)
                .aria_label(SharedString::from(name.clone()))
                .cursor_pointer()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_lg()
                .hover(|style| style.bg(rgb(RAISED)))
                .child(icon("folder").size(px(16.)).text_color(rgb(MUTED)))
                .child(
                    column()
                        .min_w_0()
                        .child(label(name).text_size(px(13.)).text_color(rgb(TEXT)))
                        .child(label(path).text_size(px(11.)).text_color(rgb(FAINT))),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.new_chat(cx) {
                        return;
                    }
                    this.project = Some(project.clone());
                    this.project_picker = false;
                    cx.notify();
                }))
                .into_any_element()
        }));
        children
    }

    fn project_browse_view(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let current = self.project_setup.listing["path"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let up = parent_of(&current);
        let entries = self.project_setup.listing["entries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let folders = entries
            .into_iter()
            .filter(|entry| entry["isDir"] == true)
            .enumerate()
            .map(|(index, entry)| {
                let name = entry["name"].as_str().unwrap_or_default().to_owned();
                let repository = entry["isRepo"] == true;
                let path = format!("{}/{name}", current.trim_end_matches('/'));
                let open = path.clone();
                row()
                    .id(("project-folder", index))
                    .role(Role::Button)
                    .aria_label(SharedString::from(name.clone()))
                    .cursor_pointer()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .hover(|style| style.bg(rgb(RAISED)))
                    .child(
                        icon(if repository { "git-branch" } else { "folder" })
                            .size(px(14.))
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        label(name)
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .text_color(rgb(TEXT)),
                    )
                    .when(repository, |view| {
                        view.child(
                            control(("project-open-repository", index), "Open")
                                .bg(rgb(RAISED))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_project_folder(open.clone(), cx);
                                })),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browse_project_folders(Some(path.clone()), cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let selected = current.clone();
        let mut children = vec![
            row()
                .gap_2()
                .when_some(up, |view, up| {
                    view.child(
                        icon_button("project-folder-up", "alt-arrow-left", "Parent folder")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.browse_project_folders(Some(up.clone()), cx);
                            })),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .child(self.project_input.clone()),
                )
                .into_any_element(),
        ];
        children.push(
            column()
                .id("project-folders")
                .max_h(px(420.))
                .overflow_y_scroll()
                .children(folders)
                .into_any_element(),
        );
        children.push(
            row()
                .justify_end()
                .child(
                    control("project-open-current", "Open this folder")
                        .bg(rgb(RAISED))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !selected.is_empty() {
                                this.open_project_folder(selected.clone(), cx);
                            }
                        })),
                )
                .into_any_element(),
        );
        children
    }

    fn project_form_view(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let setup = &self.project_setup;
        let cloning = setup.mode == SetupMode::Clone;
        let busy = setup.busy();
        let mut children = vec![];
        if cloning {
            children.push(
                self.project_field("Repository URL", &setup.url)
                    .into_any_element(),
            );
        }
        children.push(
            self.project_field(
                if cloning {
                    "Folder name (optional)"
                } else {
                    "Project name"
                },
                &setup.name,
            )
            .into_any_element(),
        );
        if !cloning {
            let templates = [
                ("Empty", ProjectTemplate::Empty),
                ("Next.js", ProjectTemplate::Nextjs),
                ("Turborepo", ProjectTemplate::Turborepo),
            ];
            children.push(
                row()
                    .gap_2()
                    .children(
                        templates
                            .into_iter()
                            .enumerate()
                            .map(|(index, (name, template))| {
                                control(("project-template", index), name)
                                    .when(setup.template == template, |view| {
                                        view.bg(rgb(0x303030)).text_color(rgb(TEXT))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.project_setup.template = template;
                                        cx.notify();
                                    }))
                            }),
                    )
                    .into_any_element(),
            );
        }
        children.push(
            self.project_field("Create in", &setup.parent)
                .into_any_element(),
        );
        children.push(
            row()
                .gap_3()
                .justify_end()
                .when_some(setup.stage.clone(), |view, stage| {
                    view.child(label(stage).text_size(px(12.)).text_color(rgb(MUTED)))
                })
                .child(
                    control("project-submit", if cloning { "Clone" } else { "Create" })
                        .bg(rgb(RAISED))
                        .when(busy, |view| view.opacity(0.5).cursor_default())
                        .on_click(cx.listener(|this, _, _, cx| this.submit_project_creation(cx))),
                )
                .into_any_element(),
        );
        children
    }

    pub(super) fn project_picker_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.project_setup.mode;
        let title = match mode {
            SetupMode::Choose => "New project",
            SetupMode::Browse => "Open folder",
            SetupMode::Clone => "Clone repository",
            SetupMode::Create => "Create project",
        };
        let body = match mode {
            SetupMode::Choose => self.project_choose_view(cx),
            SetupMode::Browse => self.project_browse_view(cx),
            SetupMode::Clone | SetupMode::Create => self.project_form_view(cx),
        };
        column()
            .id("project-setup")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .items_center()
            .child(
                column()
                    .w_full()
                    .max_w(px(640.))
                    .p_6()
                    .gap_3()
                    .child(
                        row()
                            .gap_2()
                            .when(mode != SetupMode::Choose, |view| {
                                view.child(
                                    icon_button("project-back", "alt-arrow-left", "Back").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            if !this.project_setup.busy() {
                                                this.set_project_mode(SetupMode::Choose, cx);
                                            }
                                        }),
                                    ),
                                )
                            })
                            .child(div().text_size(px(20.)).child(title))
                            .child(div().flex_1())
                            .child(
                                icon_button("close-project-picker", "close", "Close").on_click(
                                    cx.listener(|this, _, _, cx| this.close_project_setup(cx)),
                                ),
                            ),
                    )
                    .children(body)
                    .when_some(self.project_error.clone(), |view, error| {
                        view.child(
                            div()
                                .id("project-picker-error")
                                .role(Role::Alert)
                                .text_size(px(12.))
                                .text_color(rgb(0xe5a0a0))
                                .child(error),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn clone_names_come_from_any_git_url_form() {
        assert_eq!(clone_name("https://github.com/owner/repo.git"), "repo");
        assert_eq!(clone_name("https://github.com/owner/repo/"), "repo");
        assert_eq!(clone_name("git@github.com:owner/repo.git"), "repo");
    }

    #[test]
    fn folder_navigation_stops_at_the_root() {
        assert_eq!(parent_of("/home/me/code").as_deref(), Some("/home/me"));
        assert_eq!(parent_of("/home").as_deref(), Some("/"));
        assert_eq!(parent_of("/"), None);
        assert_eq!(folder_name("/home/me/code/"), "code");
    }
}
