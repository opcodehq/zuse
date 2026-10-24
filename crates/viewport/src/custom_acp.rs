//! ACP settings stay with the connected engine, including launch credentials.
use super::*;
use crate::{
    acp_command::{CommandDialog, Event},
    settings_style as s,
    shell::{MUTED, column, row},
};
use zuse_protocol::custom_acp::{CustomAcpAgent, CustomAcpAgents};
#[derive(Default)]
pub(super) struct Settings {
    snapshot: Option<CustomAcpAgents>,
    pub(super) form: Option<Entity<CommandDialog>>,
    form_events: Option<Subscription>,
    request: Option<(u64, bool)>,
    error: Option<String>,
}
impl WorkspaceView {
    pub(super) fn interrupt_custom_acp(&mut self) {
        let interrupted = self.custom_acp.request.take().is_some();
        self.custom_acp.snapshot = None;
        self.custom_acp.form = None;
        self.custom_acp.form_events = None;
        if interrupted {
            self.custom_acp.error=Some("Connection interrupted. Refresh custom agents before retrying; a save may have completed.".into());
        }
    }
    pub(super) fn refresh_custom_acp(&mut self, cx: &mut Context<Self>) {
        if self.custom_acp.form.is_none() {
            self.custom_acp_request(methods::GET_CUSTOM_ACP_AGENTS, json!({}), cx);
        }
    }
    fn custom_acp_request(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        if !self.connected || self.custom_acp.request.is_some() {
            return;
        }
        self.custom_acp.error = None;
        let id = self.call(method, params);
        self.custom_acp.request = Some((id, method != methods::GET_CUSTOM_ACP_AGENTS));
        if let Some(form) = &self.custom_acp.form {
            form.update(cx, |form, cx| form.set_status(true, None, cx));
        }
        cx.notify();
    }
    pub(super) fn receive_custom_acp(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((pending, mutation)) = self.custom_acp.request else {
            return false;
        };
        if pending != id {
            return false;
        }
        if let Some(error) = frame["err"].as_str() {
            self.custom_acp.error = Some(error.to_owned());
        } else if let Some(value) = frame.get("ok") {
            match serde_json::from_value::<CustomAcpAgents>(value.clone()) {
                Ok(snapshot) => {
                    self.custom_acp.snapshot = Some(snapshot);
                    if let Some(form) = self.custom_acp.form.take() {
                        form.update(cx, |form, cx| form.restore_focus(cx));
                    }
                    self.custom_acp.form_events = None;
                    if mutation {
                        self.call(methods::LIST_HARNESSES, json!({}));
                        self.prefetch_model_catalogs();
                    }
                }
                Err(_) => {
                    self.custom_acp.error = Some(
                        "Could not read custom agent configuration. Refresh before retrying."
                            .into(),
                    )
                }
            }
        } else {
            return true;
        }
        self.custom_acp.request = None;
        self.pending.remove(&id);
        if let Some(form) = &self.custom_acp.form {
            form.update(cx, |form, cx| {
                form.set_status(false, self.custom_acp.error.clone(), cx)
            });
        }
        cx.notify();
        true
    }
    fn edit_custom_acp(
        &mut self,
        agent: Option<CustomAcpAgent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.custom_acp.request.is_some()
            || !self.connected
            || self.custom_acp.snapshot.is_none()
            || self.custom_acp.form.is_some()
        {
            return;
        }
        let existing = agent.is_some();
        let form = cx.new(|cx| CommandDialog::new(agent, existing, window, cx));
        let events = cx.subscribe(&form, |this, form, event: &Event, cx| match event {
            Event::Save(agent) => {
                if let Some(snapshot) = &this.custom_acp.snapshot {
                    let revision = snapshot.revision;
                    this.custom_acp_request(
                        methods::SET_CUSTOM_ACP_AGENT,
                        json!({"agent":agent,"expectedRevision":revision}),
                        cx,
                    );
                } else {
                    form.update(cx, |form, cx| {
                        form.set_status(
                            false,
                            Some("Refresh custom agents before saving.".into()),
                            cx,
                        )
                    });
                }
            }
            Event::Dismiss => {
                this.custom_acp.form = None;
                this.custom_acp.form_events = None;
                cx.notify();
            }
        });
        self.custom_acp.form = Some(form);
        self.custom_acp.form_events = Some(events);
        self.custom_acp.error = None;
        cx.notify();
    }
    pub(super) fn custom_acp_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.custom_acp.request.is_some() || !self.connected;
        let editing = self.custom_acp.form.is_some();
        let agents = self
            .custom_acp
            .snapshot
            .as_ref()
            .map(|s| s.agents.clone())
            .unwrap_or_default();
        let actions = row()
            .gap(px(6.))
            .child(
                s::button(
                    "refresh-custom-acp",
                    if busy { "Working…" } else { "Refresh" },
                )
                .tab_index(0)
                .on_click(cx.listener(|this, _, _, cx| this.refresh_custom_acp(cx))),
            )
            .child(
                s::button("add-custom-acp", "Add")
                    .tab_index(0)
                    .when(busy || editing || self.custom_acp.snapshot.is_none(), |v| {
                        v.opacity(0.5).cursor_default()
                    })
                    .on_click(
                        cx.listener(|this, _, window, cx| this.edit_custom_acp(None, window, cx)),
                    ),
            );
        let card = s::card()
            .when(agents.is_empty(), |card| {
                card.child(
                    div()
                        .px(px(12.))
                        .py(px(16.))
                        .text_size(px(11.))
                        .text_color(palette::rgb(MUTED))
                        .child(if self.custom_acp.snapshot.is_some() {
                            "No custom agents yet. Add an ACP command to get started."
                        } else {
                            "Loading ACP agents…"
                        }),
                )
            })
            .when(!editing, |card| {
                card.when_some(self.custom_acp.error.clone(), |v, error| {
                    v.child(
                        s::description_text(error)
                            .text_color(s::destructive_text())
                            .p(px(12.)),
                    )
                })
            })
            .children(agents.into_iter().enumerate().map(|(index, agent)| {
                let id = agent.id;
                let command = std::iter::once(agent.command.as_str())
                    .chain(agent.args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
                let initial = agent
                    .name
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string();
                row()
                    .min_w_0()
                    .min_h(px(48.))
                    .px(px(12.))
                    .py(px(8.))
                    .gap(px(12.))
                    .when(index > 0, |v| v.border_t_1().border_color(s::card_border()))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded(px(6.))
                            .bg(s::frame_bg())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .child(initial),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(s::title_text(agent.name.clone()).truncate())
                            .child(s::description_text(command).truncate()),
                    )
                    .child(
                        s::button(("edit-custom-acp", index), "Edit")
                            .h(px(24.))
                            .tab_index(0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_custom_acp(Some(agent.clone()), window, cx)
                            })),
                    )
                    .child(
                        s::button(("remove-custom-acp", index), "Remove")
                            .h(px(24.))
                            .tab_index(0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !busy && !editing {
                                    if let Some(snapshot) = &this.custom_acp.snapshot {
                                        let revision = snapshot.revision;
                                        this.custom_acp_request(
                                            methods::DELETE_CUSTOM_ACP_AGENT,
                                            json!({"harness":id,"expectedRevision":revision}),
                                            cx,
                                        );
                                    }
                                }
                            })),
                    )
            }));
        s::frame()
            .child(s::header_with_action(
                "ACP agents",
                "",
                Some(actions.into_any_element()),
            ))
            .child(card)
            .children(self.custom_acp.form.clone())
            .into_any_element()
    }
}
