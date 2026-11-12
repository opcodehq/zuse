//! Project and chat grants, inspected through their owning engine.
use super::*;
use crate::localization::message as tr;
use crate::{icons, popover};
use zuse_i18n::messages as msg;
use zuse_protocol::permission::{
    self, PermissionAction, PermissionRule, PermissionSnapshot, RuleEffect,
};
pub(super) struct Inspector {
    pub chat: String,
    generation: uuid::Uuid,
    project_label: String,
    show_activity: bool,
    show_chat: bool,
    editing: Option<String>,
    confirming: Option<String>,
    target: Option<String>,
    snapshot: Option<PermissionSnapshot>,
    error: Option<String>,
    busy: bool,
}
impl Composer {
    pub(super) fn open_permission_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.state.read(cx).selected_chat_row().cloned() else {
            return;
        };
        let project_label = self
            .state
            .read(cx)
            .spaces
            .iter()
            .find(|space| Some(&space.id) == chat.space_id.as_ref())
            .map(|space| space.display_name().to_owned())
            .or_else(|| {
                chat.cwd
                    .as_ref()
                    .and_then(|cwd| std::path::Path::new(cwd).file_name())
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "This workspace".into());
        self.permission_open = false;
        self.permission_inspector = Some(Inspector {
            chat: chat.id,
            generation: uuid::Uuid::new_v4(),
            project_label,
            show_activity: false,
            show_chat: true,
            editing: None,
            confirming: None,
            target: Some(chat.device_id),
            snapshot: None,
            error: None,
            busy: false,
        });
        self.permission_rule_request(permission::LIST, serde_json::json!({}), cx);
    }
    fn permission_rule_request(
        &mut self,
        method: &'static str,
        mut params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(inspector) = self.permission_inspector.as_mut() else {
            return;
        };
        if inspector.busy {
            return;
        }
        inspector.busy = true;
        inspector.error = None;
        let chat = inspector.chat.clone();
        let generation = inspector.generation;
        params["chatId"] = chat.clone().into();
        if let Some(target) = &inspector.target {
            params["targetDeviceId"] = target.clone().into();
        }
        self.permission_rules_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value::<PermissionSnapshot>(value).map_err(|e| e.to_string())
                });
            this.update(cx, |this, cx| {
                if let Some(inspector) = this
                    .permission_inspector
                    .as_mut()
                    .filter(|i| i.chat == chat && i.generation == generation)
                {
                    inspector.busy = false;
                    match result {
                        Ok(snapshot) => {
                            inspector.confirming = None;
                            if inspector
                                .editing
                                .as_ref()
                                .is_some_and(|id| !snapshot.rules.iter().any(|rule| &rule.id == id))
                            {
                                inspector.editing = None;
                            }
                            inspector.snapshot = Some(snapshot);
                        }
                        Err(error) => inspector.error = Some(error),
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }
    fn close_permission_inspector(&mut self, cx: &mut Context<Self>) {
        self.permission_inspector = None;
        self.permission_rules_task = None;
        cx.notify();
    }

    pub(super) fn render_permission_inspector(
        &self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::of(cx).for_popup();
        let Some(inspector) = &self.permission_inspector else {
            return gpui::Empty.into_any_element();
        };
        let mut body = div()
            .id("permission-rules-list")
            .px(px(16.))
            .pb(px(16.))
            .max_h(px((f32::from(viewport.height) - 160.).max(120.)))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if let Some(error) = &inspector.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .mb(px(10.))
                    .child(error.clone()),
            );
        }
        if let Some(snapshot) = &inspector.snapshot {
            if snapshot.rules.is_empty() {
                body = body.child(div().py(px(12.)).text_size(px(12.)).text_color(theme.text_muted)
                    .child(tr(cx, msg::native::no_saved_permissions_yet_choose_always_allow_on_an_approval_prompt_to_save_one)));
            } else {
                let project: Vec<_> = snapshot
                    .rules
                    .iter()
                    .filter(|rule| rule.chat_id.is_none())
                    .collect();
                let chat: Vec<_> = snapshot
                    .rules
                    .iter()
                    .filter(|rule| rule.chat_id.is_some())
                    .collect();
                body = body.child(section_heading(
                    &theme,
                    if project.iter().all(|rule| rule.effect == RuleEffect::Allow) {
                        "ALWAYS ALLOWED IN THIS PROJECT"
                    } else {
                        "PROJECT RULES"
                    },
                ));
                if project.is_empty() {
                    body = body.child(empty_hint(&theme, "No project-wide allowances yet."));
                }
                for rule in project {
                    body = body.child(self.permission_rule_row(rule, &theme, cx));
                }
                body = body.child(
                    div()
                        .id("permission-chat-toggle")
                        .mt(px(16.))
                        .mb(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|view| view.text_color(theme.text))
                        .child(
                            icons::icon(if inspector.show_chat {
                                icons::ALT_ARROW_DOWN
                            } else {
                                icons::ALT_ARROW_RIGHT
                            })
                            .size(px(12.))
                            .text_color(theme.text_muted),
                        )
                        .child(format!("This chat ({})", chat.len()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(inspector) = this.permission_inspector.as_mut() {
                                inspector.show_chat = !inspector.show_chat;
                            }
                            cx.notify();
                        })),
                );
                if inspector.show_chat {
                    if chat.is_empty() {
                        body = body.child(empty_hint(&theme, "No chat overrides."));
                    }
                    for rule in chat {
                        body = body.child(self.permission_rule_row(rule, &theme, cx));
                    }
                }
            }
            body = body.child(
                div()
                    .id("permission-activity-toggle")
                    .mt(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|view| view.text_color(theme.text))
                    .child(
                        icons::icon(if inspector.show_activity {
                            icons::ALT_ARROW_DOWN
                        } else {
                            icons::ALT_ARROW_RIGHT
                        })
                        .size(px(12.))
                        .text_color(theme.text_muted),
                    )
                    .child(format!("Recent activity ({})", snapshot.activity.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(inspector) = this.permission_inspector.as_mut() {
                            inspector.show_activity = !inspector.show_activity;
                        }
                        cx.notify();
                    })),
            );
            if inspector.show_activity {
                for (index, event) in snapshot.activity.iter().enumerate() {
                    let action = event
                        .rule_id
                        .as_ref()
                        .and_then(|id| snapshot.rules.iter().find(|rule| &rule.id == id))
                        .map(|rule| PermissionAction::from_tool_input(&rule.tool, &rule.input))
                        .unwrap_or_else(|| {
                            PermissionAction::from_tool_input(&event.tool, &serde_json::Value::Null)
                        });
                    let hint = format!(
                        "{}\nDecision: {}\nRule: {}\nRequest fingerprint: {}",
                        event.source,
                        event.decision,
                        event.rule_id.as_deref().unwrap_or("None"),
                        event
                            .details
                            .get("inputHash")
                            .and_then(|value| value.as_str())
                            .unwrap_or("Not applicable")
                    );
                    let tint = match event.decision.as_str() {
                        "allow" => theme.success,
                        "deny" => theme.danger,
                        _ => theme.text_muted,
                    };
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("permission-activity-{index}")))
                            .mt(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.border.opacity(0.6))
                            .bg(theme.muted_fill().opacity(0.2))
                            .tip(hint)
                            .child(
                                icons::icon(icons::HISTORY)
                                    .size(px(14.))
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(theme.text)
                                    .child(action.detail().to_owned()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(tint)
                                    .child(event.decision.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.text_muted)
                                    .child(relative_time(event.at)),
                            ),
                    );
                }
            }
        } else {
            body = body.child(
                div()
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(theme.text_muted)
                    .child(tr(cx, msg::native::loading_permissions)),
            );
        }
        let busy = inspector.busy;
        let card = popover::dialog_card(&theme)
            .id("permission-inspector")
            .role(Role::Dialog)
            .track_focus(&self.permission_focus)
            .aria_label(format!("Permissions — {}", inspector.project_label))
            .w(px(672.))
            .max_w(px((f32::from(viewport.width) - 32.).max(280.)))
            .p(px(0.))
            .rounded(px(12.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_permission_inspector(cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_permission_inspector(cx);
                    window.focus(&this.input.focus_handle(cx), cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .p(px(16.))
                    .pb(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        icons::icon("icons/access-auto.svg")
                            .size(px(16.))
                            .text_color(permission_info_tint(&theme)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(16.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_strong)
                            .child(format!("Permissions — {}", inspector.project_label)),
                    )
                    .child(
                        icon_button(
                            &theme,
                            icons::REFRESH,
                            "Refresh permissions",
                            "permission-rules-refresh",
                        )
                        .when(busy, |view| view.opacity(0.4))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.permission_rule_request(
                                permission::LIST,
                                serde_json::json!({}),
                                cx,
                            )
                        })),
                    )
                    .child(
                        icon_button(
                            &theme,
                            icons::CLOSE,
                            "Close permissions",
                            "close-permission-inspector",
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.close_permission_inspector(cx)),
                        ),
                    ),
            )
            .child(body);
        popover::modal_glass(
            "permission-inspector-modal",
            viewport,
            card.into_any_element(),
            12.,
        )
    }

    fn permission_rule_row(
        &self,
        rule: &PermissionRule,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let inspector = self.permission_inspector.as_ref().unwrap();
        let action = PermissionAction::from_tool_input(&rule.tool, &rule.input);
        let id = rule.id.clone();
        let chat_only = rule.chat_id.is_some();
        let effect = rule.effect;
        let busy = inspector.busy;
        let tint = match effect {
            RuleEffect::Allow => {
                if chat_only {
                    theme.success
                } else {
                    permission_info_tint(theme)
                }
            }
            RuleEffect::Deny => theme.danger,
            RuleEffect::Ask => theme.warning,
        };
        let label = match effect {
            RuleEffect::Allow => {
                if chat_only {
                    "Allow in chat"
                } else {
                    "Always allow"
                }
            }
            RuleEffect::Deny => "Deny",
            RuleEffect::Ask => "Ask",
        };
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(permission_action_icon(&action, theme))
            .child(
                div()
                    .id(SharedString::from(format!("{}-resource", rule.id)))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(12.))
                    .text_color(theme.text)
                    .tip(action.detail().to_owned())
                    .child(action.detail().to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(3.))
                    .bg(tint.opacity(0.15))
                    .text_color(tint)
                    .text_size(px(10.))
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(theme.text_muted)
                    .child(relative_time(rule.updated_at)),
            );
        if inspector.confirming.as_ref() == Some(&id) {
            let revoke = id.clone();
            row = row
                .child(
                    popover::btn_ghost(
                        theme,
                        &tr(cx, msg::chat::device_bridge_panel_revoke),
                        "permission-revoke-confirm",
                    )
                    .id(SharedString::from(format!("{id}-confirm")))
                    .px(px(7.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(theme.danger)
                    .bg(theme.danger.opacity(0.12))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.permission_rule_request(
                            permission::REMOVE,
                            serde_json::json!({"id":revoke}),
                            cx,
                        )
                    })),
                )
                .child(
                    popover::btn_ghost(
                        theme,
                        &tr(cx, msg::common::cancel),
                        "permission-revoke-cancel",
                    )
                    .id(SharedString::from(format!("{id}-cancel")))
                    .px(px(7.))
                    .py(px(3.))
                    .text_size(px(10.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(inspector) = this.permission_inspector.as_mut() {
                            inspector.confirming = None;
                        }
                        cx.notify();
                    })),
                );
        } else {
            let edit = id.clone();
            let revoke = id.clone();
            row = row
                .child(
                    icon_button(
                        theme,
                        icons::MORE_HORIZONTAL,
                        "Edit permission",
                        SharedString::from(format!("{id}-edit")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(inspector) = this.permission_inspector.as_mut() {
                            if !inspector.busy {
                                inspector.editing = if inspector.editing.as_ref() == Some(&edit) {
                                    None
                                } else {
                                    Some(edit.clone())
                                };
                                inspector.confirming = None;
                            }
                        }
                        cx.notify();
                    })),
                )
                .child(
                    icon_button(
                        theme,
                        icons::TRASH_BIN_MINIMALISTIC,
                        "Revoke permission",
                        SharedString::from(format!("{id}-revoke")),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(inspector) = this.permission_inspector.as_mut() {
                            if !inspector.busy {
                                inspector.confirming = Some(revoke.clone());
                                inspector.editing = None;
                            }
                        }
                        cx.notify();
                    })),
                );
        }
        let mut item = div()
            .mt(px(4.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border.opacity(0.6))
            .bg(theme.muted_fill().opacity(0.2))
            .flex()
            .flex_col()
            .child(row)
            .when(busy, |view| view.opacity(0.6));
        if inspector.editing.as_ref() == Some(&id) {
            let mut editor = div()
                .mt(px(8.))
                .pt(px(8.))
                .border_t_1()
                .border_color(theme.border.opacity(0.6))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(4.));
            for (choice, label) in [
                (RuleEffect::Allow, "Allow"),
                (RuleEffect::Ask, "Ask"),
                (RuleEffect::Deny, "Deny"),
            ] {
                let id = id.clone();
                editor = editor.child(
                    popover::btn_ghost(theme, label, "permission-edit-effect")
                        .id(SharedString::from(format!("{id}-{label}")))
                        .text_size(px(11.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(4.))
                        .when(choice == effect, |view| view.bg(theme.muted_fill()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.permission_rule_request(
                                permission::UPDATE,
                                serde_json::json!({"id":id,"effect":choice,"chatOnly":chat_only}),
                                cx,
                            )
                        })),
                );
            }
            let scope = id.clone();
            editor = editor
                .child(div().flex_1())
                .child(
                    popover::btn_ghost(
                        theme,
                        if chat_only {
                            "Apply to project"
                        } else {
                            "Only this chat"
                        },
                        "permission-edit-scope",
                    )
                    .id(SharedString::from(format!("{id}-scope")))
                    .text_size(px(11.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.permission_rule_request(
                            permission::UPDATE,
                            serde_json::json!({"id":scope,"effect":effect,"chatOnly":!chat_only}),
                            cx,
                        )
                    })),
                )
                .child(
                    popover::btn_ghost(
                        theme,
                        &tr(cx, msg::chat::voice_stop),
                        "permission-edit-done",
                    )
                    .id(SharedString::from(format!("{id}-done")))
                    .text_size(px(11.))
                    .px(px(8.))
                    .py(px(4.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(inspector) = this.permission_inspector.as_mut() {
                            inspector.editing = None;
                        }
                        cx.notify();
                    })),
                );
            item = item.child(editor);
        }
        item.into_any_element()
    }
}

pub(super) fn permission_action_icon(action: &PermissionAction, theme: &Theme) -> gpui::Svg {
    let (icon, tint) = match action {
        PermissionAction::Command { .. } => (icons::TERMINAL, theme.warning),
        PermissionAction::FileWrite { .. } => (icons::PEN, theme.success),
        PermissionAction::Network { .. } => (icons::GLOBE, permission_info_tint(theme)),
        PermissionAction::Other { .. } => (icons::TOOL_GENERIC, theme.text_muted),
    };
    icons::icon(icon).size(px(14.)).text_color(tint)
}
fn section_heading(theme: &Theme, title: &str) -> gpui::Div {
    div()
        .mt(px(10.))
        .mb(px(4.))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(title.to_owned())
}
fn empty_hint(theme: &Theme, title: &str) -> gpui::Div {
    div()
        .mt(px(6.))
        .text_size(px(11.))
        .text_color(theme.text_muted)
        .child(title.to_owned())
}
fn icon_button(
    theme: &Theme,
    icon: &'static str,
    label: &'static str,
    id: impl Into<gpui::ElementId>,
) -> gpui::Stateful<gpui::Div> {
    let destructive = icon == icons::TRASH_BIN_MINIMALISTIC;
    let hover_fill = if destructive {
        theme.danger.opacity(0.12)
    } else {
        theme.muted_fill()
    };
    let hover_tint = if destructive {
        theme.danger
    } else {
        theme.text
    };
    let id: gpui::ElementId = id.into();
    let hover_key: SharedString = format!("permission-icon-{id:?}").into();
    let tint = motion::hover_blend(&hover_key, theme.text_muted, hover_tint);
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .tab_index(0)
        .size(px(24.))
        .flex_none()
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(tint)
        .hover(|view| view.bg(hover_fill))
        .on_hover(motion::hover_listener(hover_key))
        .tip(label.to_owned())
        .child(
            icons::icon(icon)
                .size(px(if icon == icons::CLOSE || icon == icons::REFRESH {
                    16.
                } else {
                    14.
                }))
                .text_color(tint),
        )
}
fn relative_time(at: i64) -> String {
    let seconds = (chrono::Utc::now().timestamp_millis() - at).max(0) / 1000;
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        chrono::DateTime::from_timestamp_millis(at)
            .map(|date| date.format("%b %e").to_string())
            .unwrap_or_default()
    }
}

#[cfg(feature = "appshots-fixture")]
impl Composer {
    pub fn fixture_permission_inspector(&mut self, cx: &mut Context<Self>) {
        self.open_permission_inspector(cx);
    }
    pub fn fixture_permission_rule_count(&self) -> Option<usize> {
        let inspector = self.permission_inspector.as_ref()?;
        if inspector.busy || inspector.error.is_some() {
            return None;
        }
        Some(inspector.snapshot.as_ref()?.rules.len())
    }
    pub fn fixture_permission_controls(&mut self, confirming: bool, cx: &mut Context<Self>) {
        if let Some(inspector) = self.permission_inspector.as_mut() {
            let id = inspector
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.rules.iter().find(|rule| rule.chat_id.is_none()))
                .map(|rule| rule.id.clone());
            inspector.editing = if confirming { None } else { id.clone() };
            inspector.confirming = if confirming { id } else { None };
            cx.notify();
        }
    }
    pub fn fixture_allow_project(&mut self, cx: &mut Context<Self>) {
        self.answer_permission(permission::ALLOW_PROJECT, cx);
    }
}

fn permission_info_tint(theme: &Theme) -> gpui::Hsla {
    Theme::for_preferences(theme.appearance, crate::theme::AccentColor::Blue).accent
}
