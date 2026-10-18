//! Device-scoped ACP executable setup over the same RPC used by paired clients.
use crate::{composer::ComposerInput, settings::widgets, state::AppState, theme::Theme};
use gpui::{Context, Entity, EventEmitter, Subscription, Task, Window, div, prelude::*, px};
use serde_json::json;
use zuse_protocol::custom_acp::{CustomAcpAgent, CustomAcpAgents};
use zuse_rpc::methods;

pub struct Changed;
pub struct CustomAcpCard {
    state: Entity<AppState>,
    target: Option<String>,
    snapshot: Option<CustomAcpAgents>,
    fields: Vec<Entity<ComposerInput>>,
    editing: bool,
    editing_id: Option<zuse_protocol::HarnessId>,
    error: Option<String>,
    busy: bool,
    epoch: u64,
    task: Option<Task<()>>,
    _state: Subscription,
}
impl EventEmitter<Changed> for CustomAcpCard {}
const LABELS: [&str; 6] = [
    "Provider ID (acp-name)",
    "Name",
    "Executable",
    "Arguments — one literal argument per line",
    "Environment — NAME=value per line",
    "Fallback model IDs — one per line",
];
impl CustomAcpCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.state.read(cx).engine().is_none() {
                this.clear(cx);
            } else if this.snapshot.is_none() && !this.busy {
                this.refresh(cx);
            }
        });
        let fields = LABELS
            .iter()
            .enumerate()
            .map(|(index, label)| {
                cx.new(|cx| {
                    let input = ComposerInput::new(*label, cx).with_tab_stop();
                    if index < 3 {
                        input.with_single_line()
                    } else {
                        input.with_viewport_height(72.)
                    }
                })
            })
            .collect();
        let mut card = Self {
            state,
            target: None,
            snapshot: None,
            fields,
            editing: false,
            editing_id: None,
            error: None,
            busy: false,
            epoch: 0,
            task: None,
            _state: subscription,
        };
        card.refresh(cx);
        card
    }
    fn clear(&mut self, cx: &mut Context<Self>) {
        self.epoch = self.epoch.wrapping_add(1);
        self.task = None;
        self.busy = false;
        self.snapshot = None;
        self.editing = false;
        self.editing_id = None;
        self.error = None;
        for field in &self.fields {
            field.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }
    pub fn set_target(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        if self.target == target {
            return;
        }
        self.clear(cx);
        self.target = target;
        self.refresh(cx);
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.request(methods::GET_CUSTOM_ACP_AGENTS, json!({}), cx);
    }
    fn request(
        &mut self,
        method: &'static str,
        mut params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        params["targetDeviceId"] = json!(self.target);
        let epoch = self.epoch;
        self.busy = true;
        self.error = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                card.busy = false;
                match result.and_then(|value| {
                    serde_json::from_value::<CustomAcpAgents>(value)
                        .map_err(|err| zuse_rpc::RpcError::Failed(err.to_string()))
                }) {
                    Ok(snapshot) => {
                        card.snapshot = Some(snapshot);
                        card.editing = false;
                        card.editing_id = None;
                        for field in &card.fields {
                            field.update(cx, |input, cx| input.set_text("", cx));
                        }
                        if method != methods::GET_CUSTOM_ACP_AGENTS {
                            cx.emit(Changed);
                        }
                    }
                    Err(error) => card.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn edit(&mut self, agent: Option<CustomAcpAgent>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.editing_id = agent.as_ref().map(|a| a.id);
        let values = agent
            .map(|a| {
                vec![
                    String::from(a.id),
                    a.name.clone(),
                    a.command.clone(),
                    a.argument_fields(),
                    a.environment_fields(),
                    a.model_ids.join("\n"),
                ]
            })
            .unwrap_or_else(|| {
                vec![
                    "acp-".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ]
            });
        for (field, value) in self.fields.iter().zip(values) {
            field.update(cx, |input, cx| input.set_text(value, cx));
        }
        self.editing = true;
        self.error = None;
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        let values: Vec<_> = self
            .fields
            .iter()
            .map(|field| field.read(cx).text().to_owned())
            .collect();
        match CustomAcpAgent::from_fields(
            &values[0], &values[1], &values[2], &values[3], &values[4], &values[5],
        ) {
            Ok(agent) => {
                if let Some(snapshot) = &self.snapshot {
                    self.request(
                        methods::SET_CUSTOM_ACP_AGENT,
                        json!({"agent": agent, "expectedRevision":snapshot.revision}),
                        cx,
                    );
                }
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }
}
impl Render for CustomAcpCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let button = |id: String, label: &str| {
            widgets::text_action(&theme, widgets::ActionTone::Outlined, label)
                .id(id)
                .tab_index(0)
                .role(gpui::Role::Button)
        };
        let agents = self
            .snapshot
            .as_ref()
            .map(|s| s.agents.clone())
            .unwrap_or_default();
        let busy = self.busy;
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(12.))
            .text_color(theme.text)
            .text_size(px(12.));
        if let Some(error) = &self.error {
            body = body.child(div().text_color(theme.danger_muted).child(error.clone()));
        }
        body = body.children(agents.into_iter().enumerate().map(|(index, agent)| {
            let id = agent.id;
            div().flex().items_center().justify_between().gap(px(12.))
                .child(div().flex().flex_col().child(agent.name.clone()).child(div().text_color(theme.text_muted).child(agent.command.clone())))
                .when(!busy && !self.editing, |row| row.child(div().flex().gap(px(8.))
                    .child(button(format!("edit-acp-{index}"), "Edit").on_click(cx.listener(move |this,_,_,cx|this.edit(Some(agent.clone()),cx))))
                    .child(button(format!("delete-acp-{index}"), "Remove").on_click(cx.listener(move |this,_,_,cx| {
                        if let Some(snapshot) = &this.snapshot { this.request(methods::DELETE_CUSTOM_ACP_AGENT,json!({"harness":id,"expectedRevision":snapshot.revision}),cx); }
                    })))))
        }));
        if self.editing {
            body = body.child(div().text_color(theme.text_muted).child("Runs on the selected computer. Arguments have no shell expansion. Use JSON quotes for empty or multiline values. Environment values stay in that computer’s private configuration."));
            for (index, (label, field)) in LABELS.iter().zip(&self.fields).enumerate() {
                if index == 0
                    && let Some(id) = self.editing_id
                {
                    body = body.child(div().text_color(theme.text_muted).child(String::from(id)));
                    continue;
                }
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(*label)
                        .child(field.clone()),
                );
            }
            if !busy {
                body = body.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            button("save-acp".into(), "Save agent")
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        )
                        .child(button("cancel-acp".into(), "Cancel").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.editing = false;
                                for field in &this.fields {
                                    field.update(cx, |input, cx| input.set_text("", cx));
                                }
                                cx.notify();
                            },
                        ))),
                );
            }
        } else if !busy && self.snapshot.is_some() {
            body = body.child(
                button("add-acp".into(), "Add ACP agent")
                    .on_click(cx.listener(|this, _, _, cx| this.edit(None, cx))),
            );
        }
        widgets::group_with_action(
            &theme,
            "Custom agents",
            "Connect an agent that speaks ACP over standard input and output.",
            Some(
                button(
                    "refresh-acp".into(),
                    if busy { "Working…" } else { "Refresh" },
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                .into_any_element(),
            ),
        )
        .child(widgets::section_card(&theme).mt_0().child(body))
    }
}
