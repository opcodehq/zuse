//! Per-chat permissions follow the same configuration used by Run and queues.
use super::*;
use crate::{icons, popover};

const LEVELS: [SandboxLevel; 3] = [
    SandboxLevel::WorkspaceWrite,
    SandboxLevel::DangerFullAccess,
    SandboxLevel::ReadOnly,
];
fn label(level: SandboxLevel) -> &'static str {
    match level {
        SandboxLevel::WorkspaceWrite => "Ask for approval",
        SandboxLevel::DangerFullAccess => "Full access",
        SandboxLevel::ReadOnly => "Read only",
    }
}
fn description(level: SandboxLevel) -> &'static str {
    match level {
        SandboxLevel::WorkspaceWrite => "Work in the project and ask before restricted actions.",
        SandboxLevel::DangerFullAccess => "Allow tools to run with unrestricted access.",
        SandboxLevel::ReadOnly => "Inspect files without allowing changes.",
    }
}
impl Composer {
    fn choose_permission(
        &mut self,
        level: SandboxLevel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.permission_saving.is_some()
            || self
                .permission_unconfirmed
                .as_ref()
                .is_some_and(|chat| self.state.read(cx).selected_chat.as_ref() == Some(chat))
        {
            return;
        }
        self.permission_open = false;
        window.focus(&self.input.focus_handle(cx), cx);
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            self.pickers
                .update(cx, |pickers, cx| pickers.set_draft_sandbox(level, cx));
            cx.notify();
            return;
        };
        let Some(mut config) = self.pickers.read(cx).resolved(cx).chat_config() else {
            self.failure = Some("Choose an agent before changing this chat's permissions.".into());
            self.failure_key = Some(chat_id);
            cx.notify();
            return;
        };
        let previous = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|chat| chat.config.clone());
        config.sandbox = level;
        if self.state.read(cx).side_chat_unsaved() {
            self.state.update(cx, |state, cx| {
                state.apply_chat_config(&chat_id, config);
                cx.notify();
            });
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.failure =
                Some("Connect to the execution host to change this chat's permissions.".into());
            self.failure_key = Some(chat_id);
            cx.notify();
            return;
        };
        self.permission_saving = Some(chat_id.clone());
        self.permission_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::MUTATE,
                    serde_json::json!({"op":"setChatConfig","chatId":chat_id,"config":config}),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.permission_saving = None;
                match result {
                    Ok(_) => {
                        this.state.update(cx, |state, cx| {
                            // A newer replicated config must win over this acknowledgement.
                            if state.selected_chat.as_deref() == Some(chat_id.as_str())
                                && state
                                    .selected_chat_row()
                                    .and_then(|chat| chat.config.as_ref())
                                    == previous.as_ref()
                            {
                                state.apply_chat_config(&chat_id, config);
                                cx.notify();
                            }
                        });
                    }
                    Err(error) => {
                        this.permission_unconfirmed = Some(chat_id.clone());
                        this.failure = Some(format!("Could not confirm the permission change: {error}. Open the access menu and refresh the saved mode before sending.").into());
                        this.failure_key = Some(chat_id);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn permission_blocked(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .selected_chat
            .as_ref()
            .is_some_and(|chat| {
                self.permission_saving.as_ref() == Some(chat)
                    || self.permission_unconfirmed.as_ref() == Some(chat)
            })
    }
    fn refresh_permission(&mut self, cx: &mut Context<Self>) {
        if self.permission_saving.is_some() {
            return;
        }
        let Some(chat_id) = self.permission_unconfirmed.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let previous = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|chat| chat.config.clone());
        self.permission_saving = Some(chat_id.clone());
        self.permission_task = Some(cx.spawn(async move |this, cx| {
            let read = async {
                let mut subscription = engine
                    .client()
                    .subscribe_scoped(methods::WATCH_CHATS, serde_json::json!({}))
                    .await
                    .map_err(|error| error.to_string())?;
                let snapshot = subscription
                    .recv()
                    .await
                    .ok_or_else(|| "The host closed the permission read.".to_owned())?;
                let chats: Vec<zuse_protocol::Chat> =
                    serde_json::from_value(snapshot).map_err(|error| error.to_string())?;
                chats
                    .into_iter()
                    .find(|chat| chat.id == chat_id)
                    .and_then(|chat| chat.config)
                    .ok_or_else(|| "This chat's configuration is unavailable.".to_owned())
            };
            let result = match futures::future::select(
                Box::pin(read),
                Box::pin(cx.background_executor().timer(Duration::from_secs(10))),
            )
            .await
            {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => {
                    Err("Timed out reading saved permissions.".to_owned())
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.permission_saving = None;
                match result {
                    Ok(config) if this.state.read(cx).selected_chat.as_ref() == Some(&chat_id) => {
                        this.state.update(cx, |state, cx| {
                            if state
                                .selected_chat_row()
                                .and_then(|chat| chat.config.as_ref())
                                == previous.as_ref()
                            {
                                state.apply_chat_config(&chat_id, config);
                                cx.notify();
                            }
                        });
                        this.permission_unconfirmed = None;
                        this.permission_open = false;
                        this.failure = None;
                        this.failure_key = None;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        this.failure =
                            Some(format!("Could not refresh saved permissions: {error}").into());
                        this.failure_key = Some(chat_id);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn on_permission_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.permission_saving.is_some() && event.keystroke.key != "escape" {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.permission_open = false;
                window.focus(&self.input.focus_handle(cx), cx);
            }
            "enter" | "space" if !event.is_held => {
                if self.permission_open
                    && self.permission_unconfirmed.as_ref().is_some_and(|chat| {
                        self.state.read(cx).selected_chat.as_ref() == Some(chat)
                    })
                {
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
        let pending = self.permission_saving.is_some();
        let unconfirmed = self
            .permission_unconfirmed
            .as_ref()
            .is_some_and(|chat| self.state.read(cx).selected_chat.as_ref() == Some(chat));
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
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .rounded(px(7.))
                    .text_size(crate::typography::ui_rems(11.))
                    .text_color(tint)
                    .cursor_pointer()
                    .when(pending, |view| view.opacity(0.5))
                    .hover(|view| view.bg(theme.element_hover))
                    .child(
                        icons::icon(icons::KEY_MINIMALISTIC)
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
                    .w(px(300.))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
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
                                        .text_size(crate::typography::ui_rems(12.))
                                        .child(label(level)),
                                )
                                .child(
                                    div()
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
                            .aria_label("Refresh saved permission mode")
                            .cursor_pointer()
                            .px(px(8.))
                            .py(px(8.))
                            .text_size(crate::typography::ui_rems(12.))
                            .text_color(popup_theme.accent)
                            .child(if pending {
                                "Reading saved permissions…"
                            } else {
                                "Refresh saved permission mode"
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
                assert!(composer.permission_saving.is_none());
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
