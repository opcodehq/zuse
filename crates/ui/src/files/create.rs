//! New file / new folder from the Files header: an inline name field at the
//! top of the tree. The workspace service has no create call, so this works
//! on a workspace on this computer and writes through the local filesystem;
//! the tree's watch then picks the new entry up.
use super::*;
use crate::theme::Theme;
use gpui::{AnyElement, Focusable, KeyDownEvent, MouseButton};
use gpui_base::input::{Input, InputEvent, InputState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CreateKind {
    File,
    Folder,
}

pub(super) struct TreeCreate {
    pub kind: CreateKind,
    pub input: Entity<InputState>,
    submitted: bool,
    _events: Subscription,
}

/// The workspace-relative path for a new entry named `name` at the root:
/// one or more plain segments (`a/b.txt` creates inside `a`).
pub(super) fn created_path(name: &str) -> Result<String, &'static str> {
    let name = name.trim().trim_matches('/');
    let segments: Vec<&str> = name.split('/').collect();
    if name.is_empty()
        || segments.iter().any(|segment| {
            segment.is_empty()
                || *segment == "."
                || *segment == ".."
                || segment.eq_ignore_ascii_case(".git")
                || segment.contains(['\\', '\0', ':'])
        })
    {
        return Err("Enter a name without special path segments");
    }
    Ok(segments.join("/"))
}

/// The local absolute path of `relative` under `root`, when `root` is an
/// absolute path on this computer.
fn local_path(root: &str, relative: &str) -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(root);
    root.is_absolute().then(|| root.join(relative))
}

fn create_entry(path: &std::path::Path, kind: CreateKind) -> Result<(), String> {
    if path.exists() {
        return Err("An entry with that name already exists".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    match kind {
        CreateKind::Folder => std::fs::create_dir(path).map_err(|error| error.to_string()),
        CreateKind::File => std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(drop)
            .map_err(|error| error.to_string()),
    }
}

impl FilesSurface {
    /// Whether the header's create buttons can work here.
    pub(super) fn can_create_entries(&self) -> bool {
        self.request_context.as_ref().is_some_and(|context| {
            context.target_device_id.is_none() && std::path::Path::new(&context.cwd).is_absolute()
        })
    }

    pub(super) fn begin_tree_create(
        &mut self,
        kind: CreateKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_create_entries() {
            self.report_mutation_error(
                "New files can be created only in a workspace on this computer".into(),
                cx,
            );
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(match kind {
                CreateKind::File => "File name",
                CreateKind::Folder => "Folder name",
            })
        });
        let events = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.submit_tree_create(window, cx),
            InputEvent::Blur
                if this
                    .tree_create
                    .as_ref()
                    .is_some_and(|create| !create.submitted) =>
            {
                this.tree_create = None;
                cx.notify();
            }
            _ => {}
        });
        let focus = input.focus_handle(cx);
        self.mutation_error = None;
        self.tree_create = Some(TreeCreate {
            kind,
            input,
            submitted: false,
            _events: events,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn submit_tree_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(create) = &self.tree_create else {
            return;
        };
        if create.submitted {
            return;
        }
        let kind = create.kind;
        let name = create.input.read(cx).value().to_string();
        let relative = match created_path(&name) {
            Ok(relative) => relative,
            Err(message) => {
                self.report_mutation_error(message.into(), cx);
                return;
            }
        };
        let Some(path) = self
            .request_context
            .as_ref()
            .and_then(|context| local_path(&context.cwd, &relative))
        else {
            self.tree_create = None;
            cx.notify();
            return;
        };
        self.tree_create.as_mut().unwrap().submitted = true;
        let write = cx
            .background_executor()
            .spawn(async move { create_entry(&path, kind) });
        cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| {
                this.tree_create = None;
                match result {
                    Ok(()) => {
                        this.refresh(cx);
                        this.tree_focus.focus(window, cx);
                        if kind == CreateKind::File {
                            cx.emit(FilesEvent::OpenFile(relative));
                        }
                    }
                    Err(error) => this.report_mutation_error(error.into(), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_tree_create(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let create = self.tree_create.as_ref()?;
        let theme = Theme::of(cx).clone();
        create.input.update(cx, |input, _| {
            input.set_editor_style(super::editor_adapter::editor_style(&theme));
        });
        let identity = match create.kind {
            CreateKind::File => crate::file_icons::FileIconIdentity::file("untitled"),
            CreateKind::Folder => crate::file_icons::FileIconIdentity::directory("untitled", false),
        };
        Some(
            div()
                .flex_none()
                .h(px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    crate::file_icons::icon(identity, theme.appearance)
                        .size(px(16.))
                        .flex_none(),
                )
                .child(
                    div()
                        .id("tree-create-input")
                        .debug_selector(|| "tree-create-input".into())
                        .flex_1()
                        .min_w_0()
                        .h(px(23.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.accent)
                        .bg(theme.input_glass_bg())
                        .font_family(theme.font_sans.clone())
                        .text_size(px(12.))
                        .text_color(theme.text)
                        .cursor_text()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                if this.tree_create.as_ref().is_some_and(|c| !c.submitted) {
                                    this.tree_create = None;
                                    this.tree_focus.focus(window, cx);
                                    cx.notify();
                                }
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        }))
                        .child(Input::new(&create.input)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_paths_stay_inside_the_workspace() {
        assert_eq!(created_path(" notes.md ").unwrap(), "notes.md");
        assert_eq!(created_path("src/new/a.rs").unwrap(), "src/new/a.rs");
        for invalid in ["", "/", "..", "a/../b", "a//b", ".git", "a\\b", "c:x"] {
            assert!(created_path(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn entries_are_created_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = local_path(dir.path().to_str().unwrap(), "a/b.txt").unwrap();
        create_entry(&file, CreateKind::File).unwrap();
        assert!(file.is_file());
        assert!(create_entry(&file, CreateKind::File).is_err());
        let folder = dir.path().join("c");
        create_entry(&folder, CreateKind::Folder).unwrap();
        assert!(folder.is_dir());
        assert!(local_path("~", "x").is_none());
    }
}
