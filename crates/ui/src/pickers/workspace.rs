//! Shared workspace menu presentation over the native checkout controller.
use super::*;
use zuse_protocol::presentation::WorkspaceMode;

impl Pickers {
    pub(super) fn render_creation_workspace(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.state.read(cx).cloud_project.is_some()
            || !self
                .state
                .read(cx)
                .selected_space_row()
                .is_some_and(|s| s.git_detected)
        {
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

    pub(super) fn render_checkout_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.config.checkout;
        div()
            .flex()
            .flex_col()
            .child(zuse_viewport::creation_style::workspace_heading())
            .children(
                [CheckoutKind::NewWorktree, CheckoutKind::Local]
                    .into_iter()
                    .enumerate()
                    .map(|(i, kind)| {
                        zuse_viewport::creation_style::workspace_option(
                            ("checkout-row", i),
                            kind == CheckoutKind::NewWorktree,
                            current == kind,
                            self.workspace_keyboard && self.active == i,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_checkout(kind, cx)))
                    }),
            )
            .into_any_element()
    }
}
