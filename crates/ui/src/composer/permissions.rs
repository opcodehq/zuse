//! Per-chat permissions follow the same configuration used by Run and queues.
use super::*;
use crate::{icons, popover};

use zuse_protocol::presentation::{ACCESS_LEVELS as LEVELS, access_details};
fn label(level: SandboxLevel) -> &'static str {
    access_details(level).label
}
fn description(level: SandboxLevel) -> &'static str {
    access_details(level).description
}
impl Composer {
    fn choose_permission(
        &mut self,
        level: SandboxLevel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.permission_open = false;
        window.focus(&self.input.focus_handle(cx), cx);
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            self.pickers
                .update(cx, |pickers, cx| pickers.set_draft_sandbox(level, cx));
            cx.notify();
            return;
        };
        let Some(config) = self.pickers.read(cx).resolved(cx).chat_config() else {
            self.failure = Some("Choose an agent before changing this chat's permissions.".into());
            self.failure_key = Some(chat_id);
            cx.notify();
            return;
        };
        let accepted = self.state.update(cx, |state, cx| {
            state.stage_chat_configuration(
                zuse_protocol::ChatConfigPatch::sandbox(level),
                config,
                cx,
            )
        });
        if !accepted {
            self.failure = Some(
                self.state
                    .read(cx)
                    .configuration_notice()
                    .map(|(_, notice)| notice)
                    .unwrap_or_else(|| {
                        "Refresh this chat's saved choices before changing access.".into()
                    })
                    .into(),
            );
            self.failure_key = Some(chat_id);
        }
        cx.notify();
    }
    pub(super) fn permission_blocked(&self, cx: &App) -> bool {
        self.state.read(cx).configuration_blocked()
    }
    fn refresh_permission(&mut self, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.refresh_selected_configuration(cx));
        self.permission_open = false;
        cx.notify();
    }
    fn on_permission_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).configuration_saving() && event.keystroke.key != "escape" {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.permission_open = false;
                window.focus(&self.input.focus_handle(cx), cx);
            }
            "enter" | "space" if !event.is_held => {
                if self.permission_open && self.state.read(cx).configuration_blocked() {
                    self.refresh_permission(cx);
                } else if self.permission_open {
                    self.choose_permission(LEVELS[self.permission_active], window, cx);
                } else {
                    self.permission_open = true;
                    self.permission_active = LEVELS
                        .iter()
                        .position(|level| *level == self.pickers.read(cx).resolved(cx).sandbox)
                        .unwrap_or(0);
                }
            }
            "up" | "down" if self.permission_open => {
                self.permission_active = popover::menu_step(
                    Some(self.permission_active),
                    LEVELS.len(),
                    if event.keystroke.key == "up" { -1 } else { 1 },
                )
                .unwrap_or(0);
            }
            "home" if self.permission_open => self.permission_active = 0,
            "end" if self.permission_open => self.permission_active = LEVELS.len() - 1,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(super) fn render_permission_control(
        &self,
        expanded: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self.pickers.read(cx).resolved(cx).sandbox;
        let pending = self.state.read(cx).configuration_saving();
        let unconfirmed = !pending && self.state.read(cx).configuration_blocked();
        let tint = if selected == SandboxLevel::DangerFullAccess {
            theme.warning
        } else {
            theme.text_muted
        };
        div()
            .id("composer-permissions")
            .relative()
            .flex_none()
            .track_focus(&self.permission_focus)
            .on_key_down(cx.listener(Self::on_permission_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("composer-permission-trigger")
                    .role(Role::ComboBox)
                    .aria_label(format!("Permission mode: {}", label(selected)))
                    .aria_expanded(self.permission_open)
                    .tab_index(0)
                    .h(px(28.))
                    .px(px(if expanded { 10. } else { 6. }))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(crate::typography::ui_rems(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(tint)
                    .cursor_pointer()
                    .when(pending, |view| view.opacity(0.5))
                    .hover(|view| view.bg(theme.element_hover))
                    .child(
                        icons::icon(access_details(selected).icon)
                            .size(px(14.))
                            .text_color(tint),
                    )
                    .when(expanded, |view| {
                        view.child(if pending {
                            "Saving…"
                        } else if unconfirmed {
                            "Check access"
                        } else {
                            label(selected)
                        })
                    })
                    .child(
                        icons::icon(icons::ALT_ARROW_DOWN)
                            .size(px(12.))
                            .text_color(tint),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !pending {
                            this.permission_active = LEVELS
                                .iter()
                                .position(|level| *level == selected)
                                .unwrap_or(0);
                            this.permission_open = !this.permission_open;
                            window.focus(&this.permission_focus, cx);
                            cx.notify();
                        }
                    })),
            )
            .when(self.permission_open, |view| {
                let popup_theme = theme.for_popup();
                let menu = div()
                    .id("composer-permission-menu")
                    .role(Role::ListBox)
                    .w(px(256.))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(0.))
                    .child(popover::menu_heading(&popup_theme, "Agent access"))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.permission_open = false;
                        cx.notify();
                    }))
                    .children(LEVELS.into_iter().enumerate().map(|(index, level)| {
                        popover::menu_row_nav(
                            &popup_theme,
                            level == selected,
                            index == self.permission_active,
                            format!("permission-row-{index}"),
                        )
                        .id(("permission-level", index))
                        .role(Role::ListBoxOption)
                        .aria_label(label(level))
                        .aria_selected(level == selected)
                        .h_auto()
                        .py(px(8.))
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_size(crate::typography::ui_rems(12.))
                                        .child(
                                            icons::icon(access_details(level).icon)
                                                .size(px(14.))
                                                .text_color(popup_theme.text),
                                        )
                                        .child(label(level)),
                                )
                                .child(
                                    div()
                                        .mt(px(4.))
                                        .line_height(px(16.))
                                        .text_size(crate::typography::ui_rems(11.))
                                        .text_color(popup_theme.text_muted)
                                        .child(description(level)),
                                ),
                        )
                        .when(level == selected, |view| {
                            view.child(
                                icons::icon(icons::CHECK)
                                    .size(px(14.))
                                    .text_color(popup_theme.accent),
                            )
                        })
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.choose_permission(level, window, cx),
                        ))
                    }));
                let menu = menu.when(unconfirmed, |view| {
                    view.child(
                        div()
                            .id("refresh-saved-permissions")
                            .role(Role::Button)
                            .aria_label("Refresh saved chat choices")
                            .cursor_pointer()
                            .px(px(8.))
                            .py(px(8.))
                            .text_size(crate::typography::ui_rems(12.))
                            .text_color(popup_theme.accent)
                            .child(if pending {
                                "Reading saved chat choices…"
                            } else {
                                "Refresh saved chat choices"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_permission(cx))),
                    )
                });
                view.child(popover::anchored_menu_above(
                    "composer-permission-popup",
                    menu.into_any_element(),
                    None,
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::tests::composer_focus_window;

    #[gpui::test]
    fn draft_permission_is_local_and_existing_chat_is_preserved_offline(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_directory, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, window, cx| {
                composer.choose_permission(SandboxLevel::DangerFullAccess, window, cx);
                assert_eq!(
                    composer.pickers.read(cx).resolved(cx).sandbox,
                    SandboxLevel::DangerFullAccess
                );
                assert!(!composer.state.read(cx).configuration_saving());
                assert!(composer.failure.is_none());
                composer.state.update(cx, |state, cx| {
                    state.chats = vec![
                        serde_json::from_value(serde_json::json!({
                            "id":"readonly", "deviceId":"local", "archived":false,
                            "createdAt":"2026-01-01T00:00:00Z",
                            "config":{"harness":"codex","sandbox":"read-only"}
                        }))
                        .unwrap(),
                    ];
                    state.selected_chat = Some("readonly".into());
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();
        handle
            .update(cx, |composer, window, cx| {
                assert_eq!(
                    composer.pickers.read(cx).resolved(cx).sandbox,
                    SandboxLevel::ReadOnly
                );
                composer.choose_permission(SandboxLevel::DangerFullAccess, window, cx);
                assert_eq!(
                    composer.pickers.read(cx).resolved(cx).sandbox,
                    SandboxLevel::ReadOnly
                );
                assert!(
                    composer
                        .failure
                        .as_deref()
                        .unwrap()
                        .contains("Connect to the execution host")
                );
                assert_eq!(composer.failure_key.as_deref(), Some("readonly"));
                composer.state.update(cx, |state, cx| {
                    state.selected_chat = None;
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();
        handle
            .read_with(cx, |composer, cx| {
                assert_eq!(
                    composer.pickers.read(cx).resolved(cx).sandbox,
                    SandboxLevel::WorkspaceWrite
                );
            })
            .unwrap();
    }
}
