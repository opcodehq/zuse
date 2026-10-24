//! New-chat workspace placement uses the host's existing deferred worktree run.
use super::*;
use crate::creation_style as ui;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use zuse_protocol::presentation::WorkspaceMode;

#[derive(Default)]
pub(super) struct State {
    open: Option<String>,
    active: usize,
    keyboard: bool,
    focus: RefCell<Option<FocusHandle>>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}
impl State {
    pub(super) fn dismiss(&mut self) {
        self.open = None;
    }
}

impl WorkspaceView {
    fn creation_workspace_key(&self) -> Option<String> {
        if self.cloud.mode || self.selected.is_some() {
            return None;
        }
        let path = self.project.as_ref()?.get("path")?.as_str()?;
        if path.is_empty() {
            return None;
        }
        if let Some(space) = self.spaces.iter().find(|s| s.path == path) {
            return space
                .git_detected
                .then(|| zuse_protocol::view::project_key(space));
        }
        self.repos
            .iter()
            .any(|r| r["path"] == path)
            .then(|| format!("path:{path}"))
    }

    fn creation_workspace_mode(&self) -> WorkspaceMode {
        self.creation_workspace_key()
            .and_then(|key| self.preferences.workspace_by_project.get(&key).copied())
            .unwrap_or_default()
    }

    pub(super) fn creation_worktree(&self) -> Option<zuse_protocol::WorktreeSpec> {
        self.creation_workspace_key()?;
        if self.creation_workspace_mode() != WorkspaceMode::Worktree {
            return None;
        }
        let path = self.project.as_ref()?["path"].as_str()?;
        Some(zuse_protocol::WorktreeSpec {
            repo_path: path.to_owned(),
            base: "HEAD".into(),
            space_id: self
                .spaces
                .iter()
                .find(|s| s.path == path)
                .map(|s| s.id.clone()),
        })
    }

    fn workspace_trigger_focus(&self, cx: &mut Context<Self>) -> FocusHandle {
        self.creation_checkout
            .focus
            .borrow_mut()
            .get_or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    fn close_workspace_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creation_checkout.open = None;
        window.focus(&self.workspace_trigger_focus(cx), cx);
        cx.notify();
    }

    fn choose_workspace(&mut self, worktree: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.creation_workspace_key() {
            self.preferences.workspace_by_project.insert(
                key,
                if worktree {
                    WorkspaceMode::Worktree
                } else {
                    WorkspaceMode::Local
                },
            );
            self.save_preferences();
        }
        self.close_workspace_picker(window, cx);
    }

    pub(super) fn creation_workspace_control(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.creation_workspace_key()?;
        let worktree = self.creation_workspace_mode() == WorkspaceMode::Worktree;
        let open = self.creation_checkout.open.as_ref() == Some(&key);
        let bounds = self.creation_checkout.bounds.clone();
        let focus = self.workspace_trigger_focus(cx);
        let trigger = ui::chip(
            "composer-workspace",
            if worktree { "Worktree" } else { "Local" },
            if worktree { "git-branch" } else { "folder" },
        )
        .relative()
        .track_focus(&focus)
        .aria_label("Choose workspace")
        .aria_expanded(open)
        .when(open, |v| v.bg(crate::settings_style::frame_bg()))
        .child(
            canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            if this.creation_checkout.open.as_ref() == Some(&key) {
                this.close_workspace_picker(window, cx);
            } else {
                this.cloud.picker = None;
                this.model_picker = false;
                this.access_picker = false;
                this.creation_checkout.open = Some(key.clone());
                this.creation_checkout.keyboard = false;
                this.creation_checkout.active =
                    usize::from(this.creation_workspace_mode() == WorkspaceMode::Local);
                window.focus(&this.workspace_trigger_focus(cx), cx);
                cx.notify();
            }
        }));
        let mut view = div().relative().flex_none().child(trigger);
        if open {
            let bounds = self.creation_checkout.bounds.clone();
            let menu = ui::workspace_popup()
                .id("composer-workspace-menu")
                .role(Role::Menu)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    if bounds.get().is_some_and(|b| b.contains(&e.position)) {
                        return;
                    }
                    this.creation_checkout.open = None;
                    let previous = window.focused(cx);
                    let trigger = this.workspace_trigger_focus(cx);
                    window.defer(cx, move |window, cx| {
                        if window.focused(cx) == previous {
                            window.focus(&trigger, cx);
                        }
                    });
                    cx.notify();
                }))
                .child(ui::workspace_heading())
                .children([true, false].into_iter().enumerate().map(|(i, option)| {
                    ui::workspace_option(
                        ("workspace-option", i),
                        option,
                        option == worktree,
                        self.creation_checkout.keyboard && self.creation_checkout.active == i,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_workspace(option, window, cx)
                    }))
                }));
            view = view.child(
                div().absolute().bottom_full().left_0().child(deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .offset(point(px(0.), px(-4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(menu),
                )),
            );
        }
        Some(
            view.capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if this.creation_checkout.open.as_ref() != this.creation_workspace_key().as_ref() {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "escape" => this.close_workspace_picker(window, cx),
                    "up" | "down" => {
                        this.creation_checkout.keyboard = true;
                        this.creation_checkout.active = 1 - this.creation_checkout.active;
                        cx.notify();
                    }
                    "home" => {
                        this.creation_checkout.keyboard = true;
                        this.creation_checkout.active = 0;
                        cx.notify();
                    }
                    "end" => {
                        this.creation_checkout.keyboard = true;
                        this.creation_checkout.active = 1;
                        cx.notify();
                    }
                    "enter" | "space" => {
                        this.choose_workspace(this.creation_checkout.active == 0, window, cx)
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .into_any_element(),
        )
    }
}
