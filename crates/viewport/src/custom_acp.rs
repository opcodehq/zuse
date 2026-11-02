//! ACP settings stay with the connected engine, including launch credentials.
use super::*;
use crate::acp_catalog::{CatalogDialog, Event as CatalogEvent};
use crate::acp_command::{CommandDialog, Event};
use crate::acp_rows::{Action as RowAction, AgentRows, Data as RowData, Event as RowEvent, Probe};
use zuse_protocol::custom_acp::catalog::{self, Catalog};
use zuse_protocol::custom_acp::{CustomAcpAgent, CustomAcpAgents};
#[derive(Clone)]
enum CatalogRequest {
    List,
    Icon(String),
}
pub(super) struct Settings {
    snapshot: Option<CustomAcpAgents>,
    pub(super) form: Option<Entity<CommandDialog>>,
    form_events: Option<Subscription>,
    request: Option<(u64, bool)>,
    error: Option<String>,
    catalog: Option<Entity<CatalogDialog>>,
    catalog_events: Option<Subscription>,
    catalog_open: bool,
    catalog_requests: HashMap<u64, CatalogRequest>,
    icon_queue: std::collections::VecDeque<String>,
    rows: Entity<AgentRows>,
    _row_events: Subscription,
    probes: HashMap<zuse_protocol::HarnessId, Probe>,
    probe_requests: HashMap<u64, zuse_protocol::HarnessId>,
    pending_saved: Option<zuse_protocol::HarnessId>,
    busy_label: Option<String>,
}
impl Settings {
    pub(super) fn new(window: &mut Window, cx: &mut Context<WorkspaceView>) -> Self {
        let rows = cx.new(AgentRows::new);
        let row_events = cx.subscribe_in(&rows, window, |this, _, event: &RowEvent, window, cx| {
            if event.scope == this.connection_epoch {
                this.acp_row_action(&event.action, window, cx);
            }
        });
        Self {
            snapshot: None,
            form: None,
            form_events: None,
            request: None,
            error: None,
            catalog: None,
            catalog_events: None,
            catalog_open: false,
            catalog_requests: HashMap::new(),
            icon_queue: Default::default(),
            rows,
            _row_events: row_events,
            probes: HashMap::new(),
            probe_requests: HashMap::new(),
            pending_saved: None,
            busy_label: None,
        }
    }
}
impl WorkspaceView {
    pub(super) fn interrupt_custom_acp(&mut self) {
        let interrupted = self.custom_acp.request.take().is_some();
        self.custom_acp.snapshot = None;
        self.custom_acp.form = None;
        self.custom_acp.form_events = None;
        self.custom_acp.catalog = None;
        self.custom_acp.catalog_events = None;
        self.custom_acp.catalog_open = false;
        self.custom_acp.icon_queue.clear();
        let requests = self
            .custom_acp
            .catalog_requests
            .keys()
            .copied()
            .collect::<Vec<_>>();
        self.custom_acp.catalog_requests.clear();
        for request in requests {
            self.cancel(request);
        }
        for (request, _) in std::mem::take(&mut self.custom_acp.probe_requests) {
            self.cancel(request);
        }
        self.custom_acp.probes.clear();
        self.custom_acp.pending_saved = None;
        self.custom_acp.busy_label = None;
        if interrupted {
            self.custom_acp.error=Some("Connection interrupted. Refresh custom agents before retrying; a save may have completed.".into());
        }
    }
    pub(super) fn refresh_custom_acp(&mut self, cx: &mut Context<Self>) {
        if self.custom_acp.form.is_none() && !self.custom_acp.catalog_open {
            self.custom_acp_request(methods::GET_CUSTOM_ACP_AGENTS, json!({}), false, cx);
        }
    }
    fn custom_acp_request(
        &mut self,
        method: &'static str,
        params: Value,
        test_saved: bool,
        cx: &mut Context<Self>,
    ) {
        if self.acp_control_busy() {
            return;
        }
        self.custom_acp.error = None;
        self.custom_acp.pending_saved = if test_saved {
            serde_json::from_value(params["agent"]["id"].clone()).ok()
        } else {
            None
        };
        self.custom_acp.busy_label = match method {
            methods::SET_CUSTOM_ACP_AGENT if test_saved => Some("Saving…".into()),
            methods::SET_CUSTOM_ACP_AGENT => Some("Duplicating…".into()),
            methods::DELETE_CUSTOM_ACP_AGENT => Some("Removing agent…".into()),
            _ => None,
        };
        let id = self.call(method, params);
        self.custom_acp.request = Some((id, method != methods::GET_CUSTOM_ACP_AGENTS));
        if let Some(form) = &self.custom_acp.form {
            form.update(cx, |form, cx| form.set_status(true, None, cx));
        }
        if self.custom_acp.catalog_open {
            if let Some(catalog) = &self.custom_acp.catalog {
                catalog.update(cx, |dialog, cx| dialog.set_status(true, None, cx));
            }
        }
        cx.notify();
    }
    pub(super) fn receive_custom_acp(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.receive_acp_catalog(id, frame, cx) {
            return true;
        }
        if self.receive_acp_probe(id, frame, cx) {
            return true;
        }
        let Some((pending, mutation)) = self.custom_acp.request else {
            return false;
        };
        if pending != id {
            return false;
        }
        let mut probe_after = None;
        if let Some(error) = frame["err"].as_str() {
            self.custom_acp.error = Some(error.to_owned());
        } else if let Some(value) = frame.get("ok") {
            match serde_json::from_value::<CustomAcpAgents>(value.clone()) {
                Ok(snapshot) => {
                    self.custom_acp
                        .probes
                        .retain(|id, _| snapshot.agents.iter().any(|agent| agent.id == *id));
                    probe_after = self.custom_acp.pending_saved;
                    if let Some(saved) = probe_after {
                        let retired = self
                            .custom_acp
                            .probe_requests
                            .iter()
                            .filter_map(|(request, id)| (*id == saved).then_some(*request))
                            .collect::<Vec<_>>();
                        for request in retired {
                            self.custom_acp.probe_requests.remove(&request);
                            self.cancel(request);
                        }
                        self.custom_acp.probes.remove(&saved);
                    }
                    if let Some(catalog) = &self.custom_acp.catalog {
                        catalog.update(cx, |dialog, cx| {
                            dialog.set_installed(snapshot.agents.iter().map(|agent| agent.id), cx);
                            if mutation && self.custom_acp.catalog_open {
                                dialog.restore_focus(cx);
                            }
                        });
                    }
                    if mutation {
                        self.custom_acp.catalog_open = false;
                    }
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
        self.custom_acp.busy_label = None;
        self.custom_acp.pending_saved = None;
        self.pending.remove(&id);
        if let Some(form) = &self.custom_acp.form {
            form.update(cx, |form, cx| {
                form.set_status(false, self.custom_acp.error.clone(), cx)
            });
        }
        if let Some(catalog) = &self.custom_acp.catalog {
            catalog.update(cx, |dialog, cx| {
                dialog.set_status(false, self.custom_acp.error.clone(), cx)
            });
        }
        if let Some(id) = probe_after {
            self.probe_acp(id, cx);
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
        if (self.acp_control_busy() && !self.custom_acp.catalog_open)
            || !self.connected
            || self.custom_acp.snapshot.is_none()
            || self.custom_acp.form.is_some()
        {
            return;
        }
        let existing = agent.as_ref().is_some_and(|agent| {
            self.custom_acp
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.agents.iter().any(|saved| saved.id == agent.id))
        });
        let return_focus = self
            .custom_acp
            .catalog_open
            .then(|| {
                self.custom_acp
                    .catalog
                    .as_ref()
                    .and_then(|catalog| catalog.read(cx).return_focus())
            })
            .flatten();
        let busy = self.custom_acp.request.is_some();
        let form = cx.new(|cx| {
            let mut form = CommandDialog::new(agent, existing, window, cx);
            form.set_status(busy, None, cx);
            if return_focus.is_some() {
                form.set_return_focus(return_focus);
            }
            form
        });
        let events = cx.subscribe(&form, |this, form, event: &Event, cx| match event {
            Event::Save(agent) => {
                if let Some(snapshot) = &this.custom_acp.snapshot {
                    let revision = snapshot.revision;
                    this.custom_acp_request(
                        methods::SET_CUSTOM_ACP_AGENT,
                        json!({"agent":agent,"expectedRevision":revision}),
                        true,
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
        self.custom_acp.catalog_open = false;
        self.custom_acp.form_events = Some(events);
        self.custom_acp.error = None;
        cx.notify();
    }
    fn open_acp_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connected
            || self.custom_acp.request.is_some()
            || self.custom_acp.snapshot.is_none()
            || self.custom_acp.form.is_some()
        {
            return;
        }
        if let Some(dialog) = &self.custom_acp.catalog {
            dialog.update(cx, |dialog, cx| dialog.reopen(window, cx));
        } else {
            let installed = self
                .custom_acp
                .snapshot
                .as_ref()
                .unwrap()
                .agents
                .iter()
                .map(|agent| agent.id)
                .collect::<Vec<_>>();
            let dialog = cx.new(|cx| CatalogDialog::new(installed, window, cx));
            self.custom_acp.catalog_events = Some(cx.subscribe_in(
                &dialog,
                window,
                |this, _, event: &CatalogEvent, window, cx| match event {
                    CatalogEvent::Add(agent) => {
                        if let Some(snapshot) = &this.custom_acp.snapshot {
                            this.custom_acp_request(
                                methods::SET_CUSTOM_ACP_AGENT,
                                json!({"agent":agent,"expectedRevision":snapshot.revision}),
                                true,
                                cx,
                            );
                        }
                    }
                    CatalogEvent::Manual(agent) => {
                        this.edit_custom_acp(Some(agent.clone()), window, cx)
                    }
                    CatalogEvent::Custom => this.edit_custom_acp(None, window, cx),
                    CatalogEvent::Dismiss => {
                        this.custom_acp.catalog_open = false;
                        cx.notify();
                    }
                },
            ));
            self.custom_acp.catalog = Some(dialog);
        }
        self.custom_acp.catalog_open = true;
        let request = self.call(catalog::GET, json!({}));
        self.custom_acp
            .catalog_requests
            .insert(request, CatalogRequest::List);
        cx.notify();
    }
    pub(super) fn dismiss_custom_acp(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(form) = self.custom_acp.form.clone() {
            form.update(cx, |form, cx| form.dismiss(cx));
            return true;
        }
        if self.custom_acp.catalog_open {
            if let Some(dialog) = self.custom_acp.catalog.clone() {
                dialog.update(cx, |dialog, cx| dialog.dismiss(cx));
            }
            return true;
        }
        false
    }
    fn receive_acp_catalog(&mut self, id: u64, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(kind) = self.custom_acp.catalog_requests.get(&id).cloned() else {
            return false;
        };
        if frame.get("ok").is_none() && frame["err"].as_str().is_none() {
            return true;
        }
        self.custom_acp.catalog_requests.remove(&id);
        self.pending.remove(&id);
        match kind {
            CatalogRequest::List => {
                let result = match frame["err"].as_str() {
                    Some(error) => Err(error.to_owned()),
                    None => serde_json::from_value::<Catalog>(frame["ok"].clone()).map_err(|_| {
                        "The connected computer returned an invalid ACP catalogue.".to_owned()
                    }),
                };
                if let Some(dialog) = &self.custom_acp.catalog {
                    match result {
                        Ok(data) => {
                            self.custom_acp.icon_queue = data
                                .agents
                                .iter()
                                .filter(|agent| agent.icon.is_some())
                                .map(|agent| agent.id.clone())
                                .collect();
                            dialog.update(cx, |dialog, cx| dialog.set_catalog(data, cx));
                        }
                        Err(error) => dialog
                            .update(cx, |dialog, cx| dialog.set_status(false, Some(error), cx)),
                    }
                }
            }
            CatalogRequest::Icon(registry_id) => {
                let svg = frame["ok"].as_str().map(str::to_owned);
                if let Some(dialog) = &self.custom_acp.catalog {
                    dialog.update(cx, |dialog, cx| dialog.set_icon(registry_id, svg, cx));
                }
            }
        }
        while self.custom_acp.catalog_requests.len() < 4 {
            let Some(registry_id) = self.custom_acp.icon_queue.pop_front() else {
                break;
            };
            let request = self.call(catalog::ICON, json!({"id":registry_id}));
            self.custom_acp
                .catalog_requests
                .insert(request, CatalogRequest::Icon(registry_id));
        }
        cx.notify();
        true
    }
    fn acp_control_busy(&self) -> bool {
        !self.connected
            || self.custom_acp.request.is_some()
            || self
                .provider_actions
                .values()
                .any(|id| id.starts_with("acp-"))
    }
    pub(super) fn acp_provider_error(&mut self, error: String) {
        self.custom_acp.error = Some(error);
    }
    fn acp_row_action(&mut self, action: &RowAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.acp_control_busy() {
            return;
        }
        match action {
            RowAction::Add => self.open_acp_catalog(window, cx),
            RowAction::Edit(id) => {
                let agent = self
                    .custom_acp
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.agents.iter().find(|a| a.id == *id))
                    .cloned();
                if agent.is_some() {
                    self.edit_custom_acp(agent, window, cx);
                }
            }
            RowAction::Remove(id) => {
                if let Some(snapshot) = &self.custom_acp.snapshot {
                    if snapshot.agents.iter().any(|agent| agent.id == *id) {
                        self.custom_acp_request(
                            methods::DELETE_CUSTOM_ACP_AGENT,
                            json!({"harness":id,"expectedRevision":snapshot.revision}),
                            false,
                            cx,
                        );
                    }
                }
            }
            RowAction::Toggle(id, enabled) => {
                if self
                    .custom_acp
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.agents.iter().any(|agent| agent.id == *id))
                {
                    self.custom_acp.error = None;
                    self.provider_action(
                        String::from(*id),
                        methods::SET_HARNESS_ENABLED,
                        json!({"harness":id,"enabled":enabled}),
                    );
                    cx.notify();
                }
            }
            RowAction::Probe(id) => self.probe_acp(*id, cx),
            RowAction::Duplicate(id) => {
                if let Some((mut agent, revision)) =
                    self.custom_acp.snapshot.as_ref().and_then(|s| {
                        s.agents
                            .iter()
                            .find(|agent| agent.id == *id)
                            .cloned()
                            .map(|agent| (agent, s.revision))
                    })
                {
                    agent.id = zuse_protocol::HarnessId::try_from(format!(
                        "acp-{}",
                        uuid::Uuid::new_v4().simple()
                    ))
                    .expect("generated ACP identity");
                    agent.name = format!("{} copy", agent.name);
                    self.custom_acp_request(
                        methods::SET_CUSTOM_ACP_AGENT,
                        json!({"agent":agent,"expectedRevision":revision}),
                        false,
                        cx,
                    );
                }
            }
            RowAction::Advanced(_) => {}
        }
    }
    fn probe_acp(&mut self, id: zuse_protocol::HarnessId, cx: &mut Context<Self>) {
        if self.acp_control_busy()
            || matches!(self.custom_acp.probes.get(&id), Some(Probe::Testing))
            || !self
                .custom_acp
                .snapshot
                .as_ref()
                .is_some_and(|s| s.agents.iter().any(|agent| agent.id == id))
        {
            return;
        }
        let request = self.call(methods::LIST_MODELS, json!({"harness":id,"force":true}));
        self.custom_acp.probe_requests.insert(request, id);
        self.custom_acp.probes.insert(id, Probe::Testing);
        cx.notify();
    }
    fn receive_acp_probe(&mut self, request: u64, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.custom_acp.probe_requests.get(&request).copied() else {
            return false;
        };
        if frame.get("ok").is_none() && frame["err"].as_str().is_none() {
            return true;
        }
        self.custom_acp.probe_requests.remove(&request);
        self.pending.remove(&request);
        if self
            .custom_acp
            .snapshot
            .as_ref()
            .is_some_and(|s| s.agents.iter().any(|agent| agent.id == id))
        {
            let result = if let Some(error) = frame["err"].as_str() {
                Probe::Error(error.to_owned())
            } else if serde_json::from_value::<Vec<zuse_protocol::Model>>(frame["ok"].clone())
                .is_ok()
            {
                Probe::Ready
            } else {
                Probe::Error("The agent returned an invalid model catalogue.".into())
            };
            self.custom_acp.probes.insert(id, result);
        }
        cx.notify();
        true
    }
    pub(super) fn custom_acp_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let toggling = self
            .provider_actions
            .values()
            .any(|id| id.starts_with("acp-"));
        let data = RowData {
            scope: self.connection_epoch,
            agents: self
                .custom_acp
                .snapshot
                .as_ref()
                .map(|s| s.agents.clone())
                .unwrap_or_default(),
            loaded: self.custom_acp.snapshot.is_some(),
            advanced: false,
            runtime: self.providers.clone(),
            blocked: self.acp_control_busy(),
            busy: self
                .custom_acp
                .busy_label
                .clone()
                .or_else(|| toggling.then(|| "Saving…".into())),
            error: if self.custom_acp.form.is_none() && !self.custom_acp.catalog_open {
                self.custom_acp.error.clone()
            } else {
                None
            },
            probes: self.custom_acp.probes.clone(),
        };
        self.custom_acp
            .rows
            .update(cx, |rows, cx| rows.sync(data, cx));
        div()
            .child(self.custom_acp.rows.clone())
            .children(self.custom_acp.form.clone())
            .children(
                self.custom_acp
                    .catalog_open
                    .then(|| self.custom_acp.catalog.clone())
                    .flatten(),
            )
            .into_any_element()
    }
}
