//! Project and chat grants, inspected through their owning engine.
use super::*;
use crate::popover;
use zuse_protocol::permission::{self, PermissionSnapshot, RuleEffect};
pub(super) struct Inspector {
    pub chat: String,
    generation: uuid::Uuid,
    project_label: String,
    show_activity: bool,
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
            .or_else(|| chat.cwd.clone())
            .unwrap_or_else(|| "This workspace".into());
        self.permission_open = false;
        self.permission_inspector = Some(Inspector {
            chat: chat.id,
            generation: uuid::Uuid::new_v4(),
            project_label,
            show_activity: false,
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
                        Ok(snapshot) => inspector.snapshot = Some(snapshot),
                        Err(error) => inspector.error = Some(error),
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }
    pub(super) fn render_permission_inspector(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx).clone();
        let Some(inspector) = &self.permission_inspector else {
            return gpui::Empty.into_any_element();
        };
        let busy = inspector.busy;
        let mut card = popover::popover_card(&theme)
            .id("permission-inspector")
            .role(Role::Dialog)
            .aria_label("Permission rules")
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.permission_inspector = None;
                this.permission_rules_task = None;
                cx.notify();
            }))
            .w(px(460.))
            .max_h(px(520.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(theme.text)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("Permissions · {}",inspector.project_label)),
                    )
                    .child(
                        popover::btn_ghost(&theme, "Close", "close-permission-inspector")
                            .id("close-permission-inspector")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.permission_inspector = None;
                                this.permission_rules_task = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("Exact tools and resources · chat overrides project · read only always wins"),
            )
            .child(div().text_size(px(11.)).text_color(theme.text_muted)
                .child("Rules apply to approval requests. Full access and native automatic review can run actions without a request."));
        if let Some(error) = &inspector.error {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.warning)
                    .child(error.clone()),
            );
        }
        if let Some(snapshot) = &inspector.snapshot {
            if snapshot.rules.is_empty() {
                card=card.child(div().py(px(12.)).text_size(px(12.)).text_color(theme.text_muted).child("No saved rules. Choose Always allow on an approval prompt to save one."));
            }
            let mut sorted_rules: Vec<_> = snapshot.rules.iter().collect();
            sorted_rules.sort_by_key(|rule| rule.chat_id.is_some());
            let mut previous_scope = None;
            for rule in sorted_rules {
                let scope = rule.chat_id.is_some();
                if previous_scope != Some(scope) {
                    card = card.child(popover::menu_heading(
                        &theme,
                        if scope {
                            "This chat overrides"
                        } else {
                            "Project rules"
                        },
                    ));
                    previous_scope = Some(scope);
                }
                let id = rule.id.clone();
                let chat_only = rule.chat_id.is_some();
                let mut controls = div().flex().items_center().gap(px(4.));
                for (effect, label) in [
                    (RuleEffect::Allow, "Allow"),
                    (RuleEffect::Ask, "Ask"),
                    (RuleEffect::Deny, "Deny"),
                ] {
                    let id = id.clone();
                    controls=controls.child(popover::btn_ghost(&theme,label,"permission-rule-effect")
                        .id(SharedString::from(format!("{id}-{label}")))
                        .when(rule.effect==effect,|v|v.bg(theme.muted_fill()))
                        .when(busy,|v|v.opacity(0.5))
                        .on_click(cx.listener(move|this,_,_,cx|this.permission_rule_request(permission::UPDATE,serde_json::json!({"id":id,"effect":effect,"chatOnly":chat_only}),cx))));
                }
                let scope_id = id.clone();
                let effect = rule.effect;
                controls=controls.child(popover::btn_ghost(&theme,if chat_only{"Move to project"}else{"Move to this chat"},"permission-rule-scope")
                    .id(SharedString::from(format!("{id}-scope")))
                    .on_click(cx.listener(move|this,_,_,cx|this.permission_rule_request(permission::UPDATE,serde_json::json!({"id":scope_id,"effect":effect,"chatOnly":!chat_only}),cx))))
                    .child(popover::btn_ghost(&theme,"Remove","permission-rule-remove").id(SharedString::from(format!("{id}-remove")))
                        .on_click(cx.listener(move|this,_,_,cx|this.permission_rule_request(permission::REMOVE,serde_json::json!({"id":id}),cx))));
                card = card.child(
                    div()
                        .border_t_1()
                        .border_color(theme.border)
                        .pt(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.text)
                                .child(format!(
                                    "{} · {}",
                                    rule.tool,
                                    if chat_only { "This chat" } else { "Project" }
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.text_muted)
                                .child(rule.input.to_string()),
                        )
                        .child(controls),
                );
            }
            card = card.child(
                popover::btn_ghost(
                    &theme,
                    &format!(
                        "{} Recent activity ({})",
                        if inspector.show_activity {
                            "▾"
                        } else {
                            "▸"
                        },
                        snapshot.activity.len()
                    ),
                    "permission-activity-toggle",
                )
                .id("permission-activity-toggle")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(inspector) = this.permission_inspector.as_mut() {
                        inspector.show_activity = !inspector.show_activity;
                    }
                    cx.notify();
                })),
            );
            for (index, event) in snapshot
                .activity
                .iter()
                .enumerate()
                .filter(|_| inspector.show_activity)
            {
                card = card.child(
                    div()
                        .id(("permission-activity", index))
                        .tooltip(crate::settings::widgets::text_tooltip(
                            event.details.to_string(),
                        ))
                        .text_size(px(11.))
                        .text_color(theme.text_muted)
                        .child(format!(
                            "{} · {} · {} · {}{}",
                            chrono::DateTime::from_timestamp_millis(event.at)
                                .map(|at| at.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                                .unwrap_or_else(|| event.at.to_string()),
                            event.tool,
                            event.decision,
                            event.source,
                            event
                                .rule_id
                                .as_ref()
                                .map(|id| format!(" · {id}"))
                                .unwrap_or_default()
                        )),
                );
            }
        }
        card = card.child(
            popover::btn_ghost(
                &theme,
                if busy { "Loading…" } else { "Refresh" },
                "permission-rules-refresh",
            )
            .id("permission-rules-refresh")
            .on_click(cx.listener(|this, _, _, cx| {
                this.permission_rule_request(permission::LIST, serde_json::json!({}), cx)
            })),
        );
        popover::anchored_menu_above("permission-inspector-popup", card.into_any_element(), None)
            .into_any_element()
    }
}
