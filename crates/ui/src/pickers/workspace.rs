//! Shared workspace menu presentation over the native checkout controller.
use super::*;
use zuse_protocol::presentation::WorkspaceMode;

/// One row of the workspace menu. Cloud isn't one: where a chat runs is the
/// computer picker's choice, and a cloud chat always gets a fresh branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceChoice {
    Worktree,
    Local,
}

impl Pickers {
    /// The Worktree / Local chip, shown only for a chat on this computer.
    pub(super) fn render_creation_workspace(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.shows_workspace_chip(cx) {
            return None;
        }
        self.ensure_refs(false, cx);
        let theme = Theme::of(cx).clone();
        let mut trigger = self
            .tray_chip(
                PickerKind::Checkout,
                "picker-checkout",
                if self.config.checkout == CheckoutKind::NewWorktree {
                    crate::icons::DITHER_BRANCH
                } else {
                    crate::icons::DITHER_FOLDER
                },
                self.checkout_label().into(),
                &theme,
                cx,
            )
            .flex_none();
        if self.mounted_kind() == Some(PickerKind::Checkout) {
            let content = self.render_checkout_popover(cx);
            let popup = self.popover_frame(224., content, cx);
            trigger = trigger.child(popover::anchored_menu_above(
                "checkout-popover",
                popup,
                self.open.closing_since(),
            ));
        }
        Some(trigger.into_any_element())
    }

    /// Worktree or Local only means something for a checkout on this side;
    /// a chat bound for Cloud has no workspace chip.
    pub(super) fn shows_workspace_chip(&self, cx: &App) -> bool {
        self.cloud.task_binding.is_none()
            && self.state.read(cx).cloud_project.is_none()
            && self.workspace_choices(cx).len() >= 2
    }

    pub(super) fn remembered_checkout(
        state: &Entity<AppState>,
        defaults: &ComposerDefaults,
        cx: &App,
    ) -> CheckoutKind {
        let Some(space) = state
            .read(cx)
            .selected_space_row()
            .filter(|s| s.git_detected)
        else {
            return CheckoutKind::Local;
        };
        match defaults
            .workspace_by_project
            .get(&zuse_protocol::view::project_key(space))
            .copied()
            .unwrap_or_default()
        {
            WorkspaceMode::Worktree => CheckoutKind::NewWorktree,
            WorkspaceMode::Local => CheckoutKind::Local,
        }
    }

    /// The workspace menu's rows: Worktree and Local for a git checkout.
    pub(super) fn workspace_choices(&self, cx: &App) -> Vec<WorkspaceChoice> {
        if self
            .state
            .read(cx)
            .selected_space_row()
            .is_some_and(|s| s.git_detected)
        {
            vec![WorkspaceChoice::Worktree, WorkspaceChoice::Local]
        } else {
            Vec::new()
        }
    }

    /// The selected row of the workspace menu.
    pub(super) fn workspace_choice(&self) -> WorkspaceChoice {
        if self.config.checkout == CheckoutKind::NewWorktree {
            WorkspaceChoice::Worktree
        } else {
            WorkspaceChoice::Local
        }
    }

    pub(super) fn pick_workspace(&mut self, choice: WorkspaceChoice, cx: &mut Context<Self>) {
        self.pick_checkout(
            if choice == WorkspaceChoice::Worktree {
                CheckoutKind::NewWorktree
            } else {
                CheckoutKind::Local
            },
            cx,
        );
    }

    pub(super) fn render_checkout_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.workspace_choice();
        div()
            .flex()
            .flex_col()
            .child(zuse_viewport::creation_style::workspace_heading())
            .children(
                self.workspace_choices(cx)
                    .into_iter()
                    .enumerate()
                    .map(|(i, choice)| {
                        let active = self.workspace_keyboard && self.active == i;
                        zuse_viewport::creation_style::workspace_option(
                            ("checkout-row", i),
                            choice == WorkspaceChoice::Worktree,
                            current == choice,
                            active,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.pick_workspace(choice, cx)),
                        )
                    }),
            )
            .into_any_element()
    }
}
