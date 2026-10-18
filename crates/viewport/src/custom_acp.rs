//! ACP settings are private to the connected engine, including launch credentials.
use super::*;
use crate::{
    settings_style as s,
    shell::{column, row},
};
use zuse_protocol::custom_acp::{CustomAcpAgent, CustomAcpAgents};

#[derive(Default)]
pub(super) struct Settings {
    snapshot: Option<CustomAcpAgents>,
    fields: Option<Vec<Entity<input::TextInput>>>,
    editing_id: Option<zuse_protocol::HarnessId>,
    request: Option<(u64, bool)>,
    error: Option<String>,
}
const LABELS: [&str; 6] = [
    "Provider ID (acp-name)",
    "Name",
    "Executable",
    "Arguments — one literal argument per line",
    "Environment — NAME=value per line",
    "Fallback model IDs — one per line",
];
impl WorkspaceView {
    pub(super) fn interrupt_custom_acp(&mut self) {
        let interrupted = self.custom_acp.request.take().is_some();
        self.custom_acp.snapshot = None;
        self.custom_acp.fields = None;
        if interrupted {
            self.custom_acp.error = Some("Connection interrupted. Refresh custom agents before retrying; a save may have completed.".into());
        }
    }
    fn custom_acp_request(&mut self, method: &'static str, params: Value) {
        if !self.connected || self.custom_acp.request.is_some() {
            return;
        }
        let id = self.call(method, params);
        self.custom_acp.request = Some((id, method != methods::GET_CUSTOM_ACP_AGENTS));
        self.custom_acp.error = None;
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
                    self.custom_acp.fields = None;
                    self.custom_acp.editing_id = None;
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
        cx.notify();
        true
    }
    fn edit_custom_acp(
        &mut self,
        agent: Option<CustomAcpAgent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.custom_acp.request.is_some() || !self.connected {
            return;
        }
        self.custom_acp.editing_id = agent.as_ref().map(|a| a.id);
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
        self.custom_acp.fields = Some(
            LABELS
                .iter()
                .enumerate()
                .zip(values)
                .map(|((index, label), value)| {
                    cx.new(|cx| {
                        let mut input = if index < 3 {
                            input::TextInput::new(label, window, cx)
                        } else {
                            input::TextInput::multiline(label, window, cx)
                        };
                        input.set_value(value, cx);
                        input
                    })
                })
                .collect(),
        );
        self.custom_acp.error = None;
        cx.notify();
    }
    fn save_custom_acp(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.custom_acp.fields else {
            return;
        };
        let values: Vec<_> = fields.iter().map(|f| f.read(cx).value(cx)).collect();
        match CustomAcpAgent::from_fields(
            &values[0], &values[1], &values[2], &values[3], &values[4], &values[5],
        ) {
            Ok(agent) => {
                if let Some(snapshot) = &self.custom_acp.snapshot {
                    self.custom_acp_request(
                        methods::SET_CUSTOM_ACP_AGENT,
                        json!({"agent":agent,"expectedRevision":snapshot.revision}),
                    );
                }
            }
            Err(error) => self.custom_acp.error = Some(error),
        }
        cx.notify();
    }
    pub(super) fn custom_acp_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.custom_acp.request.is_some() || !self.connected;
        let editing = self.custom_acp.fields.is_some();
        let agents = self
            .custom_acp
            .snapshot
            .as_ref()
            .map(|s| s.agents.clone())
            .unwrap_or_default();
        let mut body = column().gap(px(12.)).p(px(12.));
        if let Some(error) = &self.custom_acp.error {
            body =
                body.child(s::description_text(error.clone()).text_color(palette::rgb(0xf87171)));
        }
        for (index, agent) in agents.into_iter().enumerate() {
            let id = agent.id;
            body = body.child(row().justify_between().gap(px(12.))
                .child(column().child(s::title_text(agent.name.clone())).child(s::description_text(agent.command.clone())))
                .when(!busy && !editing, |r| r.child(row().gap(px(8.))
                    .child(s::button(("edit-custom-acp",index),"Edit").on_click(cx.listener(move |this,_,window,cx|this.edit_custom_acp(Some(agent.clone()),window,cx))))
                    .child(s::button(("remove-custom-acp",index),"Remove").on_click(cx.listener(move |this,_,_,cx| {
                        if let Some(snapshot) = &this.custom_acp.snapshot { this.custom_acp_request(methods::DELETE_CUSTOM_ACP_AGENT,json!({"harness":id,"expectedRevision":snapshot.revision})); } cx.notify();
                    }))))));
        }
        if let Some(fields) = &self.custom_acp.fields {
            body = body.child(s::description_text("Runs on the connected computer. Arguments have no shell expansion. Use JSON quotes for empty or multiline values. Environment values stay in that computer’s private configuration."));
            for (index, (label, field)) in LABELS.iter().zip(fields).enumerate() {
                if index == 0
                    && let Some(id) = self.custom_acp.editing_id
                {
                    body = body.child(s::description_text(String::from(id)));
                    continue;
                }
                body = body.child(
                    column()
                        .gap(px(6.))
                        .child(s::title_text(*label))
                        .child(field.clone()),
                );
            }
            if !busy {
                body = body.child(
                    row()
                        .gap(px(8.))
                        .child(
                            s::button("save-custom-acp", "Save agent")
                                .on_click(cx.listener(|this, _, _, cx| this.save_custom_acp(cx))),
                        )
                        .child(
                            s::button("cancel-custom-acp", "Cancel").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.custom_acp.fields = None;
                                    cx.notify();
                                },
                            )),
                        ),
                );
            }
        } else if !busy && self.custom_acp.snapshot.is_some() {
            body = body.child(s::button("add-custom-acp", "Add ACP agent").on_click(
                cx.listener(|this, _, window, cx| this.edit_custom_acp(None, window, cx)),
            ));
        }
        s::frame()
            .child(
                s::header(
                    "Custom agents",
                    "Connect an agent that speaks ACP over standard input and output.",
                )
                .child(
                    s::button(
                        "refresh-custom-acp",
                        if busy { "Working…" } else { "Refresh" },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.custom_acp_request(methods::GET_CUSTOM_ACP_AGENTS, json!({}));
                        cx.notify();
                    })),
                ),
            )
            .child(s::card().child(body))
            .into_any_element()
    }
}
