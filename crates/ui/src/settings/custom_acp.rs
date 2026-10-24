//! ACP configuration belongs to the selected engine; presentation is shared.
use crate::state::AppState;
#[cfg(feature = "browser-fixture")]
use acp_registry::{Offer, RegistryAgent};
use futures::StreamExt;
use gpui::{prelude::*, *};
use serde_json::json;
use std::collections::HashMap;
#[cfg(feature = "browser-fixture")]
use zuse_protocol::custom_acp::catalog as acp_registry;
use zuse_protocol::custom_acp::{
    CustomAcpAgent, CustomAcpAgents,
    catalog::{self, Catalog},
};
use zuse_rpc::methods;
use zuse_viewport::acp_rows::{
    Action as RowAction, AgentRows, Data as RowData, Event as RowEvent, Probe,
};
use zuse_viewport::{
    acp_catalog::{CatalogDialog, Event as CatalogEvent},
    acp_command::{CommandDialog, Event as CommandEvent},
};

pub enum Changed {
    Reload,
    Advanced(zuse_protocol::HarnessId),
}
pub struct CustomAcpCard {
    state: Entity<AppState>,
    target: Option<String>,
    snapshot: Option<CustomAcpAgents>,
    form: Option<Entity<CommandDialog>>,
    form_events: Option<Subscription>,
    catalog: Option<Entity<CatalogDialog>>,
    catalog_events: Option<Subscription>,
    dialog_open: bool,
    catalog_task: Option<Task<()>>,
    error: Option<String>,
    busy: bool,
    epoch: u64,
    task: Option<Task<()>>,
    fixture: bool,
    fixture_open: bool,
    fixture_catalog: Option<Catalog>,
    _state: Subscription,
    rows: Entity<AgentRows>,
    row_events: Option<Subscription>,
    runtime: Vec<serde_json::Value>,
    probes: HashMap<zuse_protocol::HarnessId, Probe>,
    probe_tasks: HashMap<zuse_protocol::HarnessId, Task<()>>,
    busy_label: Option<String>,
}
impl EventEmitter<Changed> for CustomAcpCard {}
impl CustomAcpCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.fixture {
                return;
            }
            if this.state.read(cx).engine().is_none() {
                this.clear(cx);
            } else if this.snapshot.is_none() && !this.busy {
                this.refresh(cx);
            }
        });
        let mut card = Self {
            state,
            target: None,
            snapshot: None,
            form: None,
            form_events: None,
            catalog: None,
            catalog_events: None,
            dialog_open: false,
            catalog_task: None,
            error: None,
            busy: false,
            epoch: 0,
            task: None,
            fixture: false,
            fixture_open: false,
            fixture_catalog: None,
            _state: subscription,
            rows: cx.new(AgentRows::new),
            row_events: None,
            runtime: Vec::new(),
            probes: HashMap::new(),
            probe_tasks: HashMap::new(),
            busy_label: None,
        };
        card.refresh(cx);
        card
    }
    fn clear(&mut self, cx: &mut Context<Self>) {
        self.epoch = self.epoch.wrapping_add(1);
        self.task = None;
        self.catalog_task = None;
        self.busy = false;
        self.snapshot = None;
        self.form = None;
        self.form_events = None;
        self.catalog = None;
        self.catalog_events = None;
        self.dialog_open = false;
        self.error = None;
        self.runtime.clear();
        self.probes.clear();
        self.probe_tasks.clear();
        self.busy_label = None;
        cx.notify();
    }
    pub fn set_target(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        if self.target == target {
            if !self.busy && self.form.is_none() && !self.dialog_open {
                self.refresh(cx);
            }
            return;
        }
        self.clear(cx);
        self.target = target;
        self.refresh(cx);
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.request(methods::GET_CUSTOM_ACP_AGENTS, json!({}), false, cx);
    }
    fn request(
        &mut self,
        method: &'static str,
        mut params: serde_json::Value,
        test_saved: bool,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error = Some("The selected computer is not connected.".into());
            self.sync_status(cx);
            return;
        };
        params["targetDeviceId"] = json!(self.target);
        let epoch = self.epoch;
        let target = self.target.clone();
        let saved_id = (test_saved && method == methods::SET_CUSTOM_ACP_AGENT)
            .then(|| {
                serde_json::from_value::<zuse_protocol::HarnessId>(params["agent"]["id"].clone())
                    .ok()
            })
            .flatten();
        self.busy = true;
        self.busy_label = match method {
            methods::SET_CUSTOM_ACP_AGENT if test_saved => Some("Saving…".into()),
            methods::SET_CUSTOM_ACP_AGENT => Some("Duplicating…".into()),
            methods::DELETE_CUSTOM_ACP_AGENT => Some("Removing agent…".into()),
            _ => None,
        };
        self.error = None;
        self.sync_status(cx);
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            let runtime = if result.is_ok() {
                engine
                    .client()
                    .call(methods::LIST_HARNESSES, json!({"targetDeviceId":target}))
                    .await
                    .ok()
                    .and_then(|value| value.as_array().cloned())
            } else {
                None
            };
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                card.busy = false;
                card.busy_label = None;
                if let Some(runtime) = runtime {
                    card.runtime = runtime;
                }
                match result.and_then(|value| {
                    serde_json::from_value::<CustomAcpAgents>(value)
                        .map_err(|error| zuse_rpc::RpcError::Failed(error.to_string()))
                }) {
                    Ok(snapshot) => {
                        card.probes
                            .retain(|id, _| snapshot.agents.iter().any(|agent| agent.id == *id));
                        if let Some(id) = saved_id {
                            card.probe_tasks.remove(&id);
                            card.probes.remove(&id);
                        }
                        if let Some(catalog) = &card.catalog {
                            catalog.update(cx, |dialog, cx| {
                                dialog
                                    .set_installed(snapshot.agents.iter().map(|agent| agent.id), cx)
                            });
                        }
                        card.snapshot = Some(snapshot);
                        if let Some(form) = card.form.take() {
                            form.update(cx, |form, cx| form.restore_focus(cx));
                        }
                        card.form_events = None;
                        if method != methods::GET_CUSTOM_ACP_AGENTS {
                            if card.dialog_open {
                                if let Some(catalog) = &card.catalog {
                                    catalog.update(cx, |dialog, cx| dialog.restore_focus(cx));
                                }
                            }
                            card.dialog_open = false;
                            cx.emit(Changed::Reload);
                        }
                        if let Some(id) = saved_id {
                            card.probe(id, cx);
                        }
                    }
                    Err(error) => card.error = Some(error.to_string()),
                }
                card.sync_status(cx);
            });
        }));
    }
    fn sync_status(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &self.form {
            form.update(cx, |form, cx| {
                form.set_status(self.busy, self.error.clone(), cx)
            });
        }
        if let Some(catalog) = &self.catalog {
            catalog.update(cx, |dialog, cx| {
                dialog.set_status(self.busy, self.error.clone(), cx)
            });
        }
        cx.notify();
    }
    fn installed(&self, id: zuse_protocol::HarnessId) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.agents.iter().any(|agent| agent.id == id))
    }
    fn row_action(&mut self, action: &RowAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            RowAction::Add => self.open_dialog(window, cx),
            RowAction::Edit(id) => {
                let agent = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.agents.iter().find(|a| a.id == *id))
                    .cloned();
                if agent.is_some() {
                    self.edit(agent, window, cx);
                }
            }
            RowAction::Remove(id) => self.remove(*id, cx),
            RowAction::Toggle(id, enabled) => self.toggle(*id, *enabled, cx),
            RowAction::Probe(id) => self.probe(*id, cx),
            RowAction::Advanced(id) => cx.emit(Changed::Advanced(*id)),
            RowAction::Duplicate(id) => {
                if self.busy {
                    return;
                }
                if let Some(mut agent) = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.agents.iter().find(|a| a.id == *id))
                    .cloned()
                {
                    agent.id = zuse_protocol::HarnessId::try_from(format!(
                        "acp-{}",
                        uuid::Uuid::new_v4().simple()
                    ))
                    .expect("generated ACP identity");
                    agent.name = format!("{} copy", agent.name);
                    if let Some(snapshot) = &self.snapshot {
                        self.request(
                            methods::SET_CUSTOM_ACP_AGENT,
                            json!({"agent":agent,"expectedRevision":snapshot.revision}),
                            false,
                            cx,
                        );
                    }
                }
            }
        }
    }
    fn toggle(&mut self, id: zuse_protocol::HarnessId, enabled: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target.clone();
        let epoch = self.epoch;
        self.busy = true;
        self.busy_label = Some("Saving…".into());
        self.error = None;
        self.sync_status(cx);
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::SET_HARNESS_ENABLED,
                    json!({"harness":id,"enabled":enabled,"targetDeviceId":target}),
                )
                .await;
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                card.busy = false;
                card.busy_label = None;
                match result {
                    Ok(value) => {
                        if let Some(runtime) = value.as_array() {
                            card.runtime = runtime.clone();
                        }
                        cx.emit(Changed::Reload);
                    }
                    Err(error) => card.error = Some(error.to_string()),
                }
                card.sync_status(cx);
            });
        }));
    }
    fn probe(&mut self, id: zuse_protocol::HarnessId, cx: &mut Context<Self>) {
        if self.busy || matches!(self.probes.get(&id), Some(Probe::Testing)) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let epoch = self.epoch;
        let target = self.target.clone();
        self.probes.insert(id, Probe::Testing);
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::LIST_MODELS,
                    json!({"harness":id,"force":true,"targetDeviceId":target}),
                )
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<zuse_protocol::Model>>(value)
                        .map_err(|_| "The agent returned an invalid model catalogue.".into())
                });
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch || !card.installed(id) {
                    return;
                }
                card.probes.insert(
                    id,
                    match result {
                        Ok(_) => Probe::Ready,
                        Err(error) => Probe::Error(error),
                    },
                );
                card.probe_tasks.remove(&id);
                cx.notify();
            });
        });
        self.probe_tasks.insert(id, task);
    }
    fn put(&mut self, agent: CustomAcpAgent, cx: &mut Context<Self>) {
        if let Some(snapshot) = &self.snapshot {
            self.request(
                methods::SET_CUSTOM_ACP_AGENT,
                json!({"agent":agent,"expectedRevision":snapshot.revision}),
                true,
                cx,
            );
        } else {
            self.error = Some("Custom agents are still loading. Try again in a moment.".into());
            self.busy = false;
            self.sync_status(cx);
        }
    }
    fn remove(&mut self, id: zuse_protocol::HarnessId, cx: &mut Context<Self>) {
        if let Some(snapshot) = &self.snapshot {
            self.request(
                methods::DELETE_CUSTOM_ACP_AGENT,
                json!({"harness":id,"expectedRevision":snapshot.revision}),
                false,
                cx,
            );
        }
    }
    fn edit(&mut self, agent: Option<CustomAcpAgent>, window: &mut Window, cx: &mut Context<Self>) {
        if (self.busy && !self.dialog_open) || self.form.is_some() {
            return;
        }
        let existing = agent.as_ref().is_some_and(|agent| self.installed(agent.id));
        let return_focus = self
            .dialog_open
            .then(|| {
                self.catalog
                    .as_ref()
                    .and_then(|catalog| catalog.read(cx).return_focus())
            })
            .flatten();
        let busy = self.busy;
        let form = cx.new(|cx| {
            let mut form = CommandDialog::new(agent, existing, window, cx);
            form.set_status(busy, None, cx);
            if return_focus.is_some() {
                form.set_return_focus(return_focus);
            }
            form
        });
        self.form_events = Some(cx.subscribe(
            &form,
            |this, _, event: &CommandEvent, cx| match event {
                CommandEvent::Save(agent) => this.put(agent.clone(), cx),
                CommandEvent::Dismiss => {
                    this.form = None;
                    this.form_events = None;
                    cx.notify();
                }
            },
        ));
        self.dialog_open = false;
        self.form = Some(form);
        self.error = None;
        cx.notify();
    }
    pub(super) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(form) = self.form.clone() {
            form.update(cx, |form, cx| form.dismiss(cx));
            return true;
        }
        if self.dialog_open {
            if let Some(catalog) = self.catalog.clone() {
                catalog.update(cx, |dialog, cx| dialog.dismiss(cx));
            }
            return true;
        }
        false
    }
    fn open_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.form.is_some() {
            return;
        }
        if let Some(catalog) = &self.catalog {
            catalog.update(cx, |dialog, cx| dialog.reopen(window, cx));
        } else {
            let installed = self
                .snapshot
                .as_ref()
                .map(|s| s.agents.iter().map(|a| a.id).collect::<Vec<_>>())
                .unwrap_or_default();
            let catalog = cx.new(|cx| CatalogDialog::new(installed, window, cx));
            self.catalog_events = Some(cx.subscribe_in(
                &catalog,
                window,
                |this, _, event: &CatalogEvent, window, cx| match event {
                    CatalogEvent::Add(agent) => this.put(agent.clone(), cx),
                    CatalogEvent::Manual(agent) => this.edit(Some(agent.clone()), window, cx),
                    CatalogEvent::Custom => this.edit(None, window, cx),
                    CatalogEvent::Dismiss => {
                        this.dialog_open = false;
                        cx.notify();
                    }
                },
            ));
            self.catalog = Some(catalog);
        }
        self.dialog_open = true;
        if self.fixture {
            if let (Some(dialog), Some(data)) = (&self.catalog, &self.fixture_catalog) {
                dialog.update(cx, |dialog, cx| dialog.set_catalog(data.clone(), cx));
            }
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target.clone();
        let epoch = self.epoch;
        self.catalog_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(catalog::GET, json!({"targetDeviceId":target}))
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Catalog>(value).map_err(|error| error.to_string())
                });
            let ids = match &result {
                Ok(data) => data
                    .agents
                    .iter()
                    .filter(|agent| agent.icon.is_some())
                    .map(|agent| agent.id.clone())
                    .collect::<Vec<_>>(),
                Err(_) => Vec::new(),
            };
            let live = this
                .update(cx, |card, cx| {
                    if card.epoch != epoch {
                        return false;
                    }
                    if let Some(dialog) = &card.catalog {
                        dialog.update(cx, |dialog, cx| match result {
                            Ok(data) => dialog.set_catalog(data, cx),
                            Err(error) => dialog.set_status(false, Some(error), cx),
                        });
                    }
                    true
                })
                .unwrap_or(false);
            if !live {
                return;
            }
            let mut icons = futures::stream::iter(ids)
                .map(|id| {
                    let engine = engine.clone();
                    let target = target.clone();
                    async move {
                        let result = engine
                            .client()
                            .call(catalog::ICON, json!({"id":id,"targetDeviceId":target}))
                            .await;
                        (
                            id,
                            result
                                .ok()
                                .and_then(|value| value.as_str().map(str::to_owned)),
                        )
                    }
                })
                .buffer_unordered(4);
            while let Some((id, svg)) = icons.next().await {
                let live = this
                    .update(cx, |card, cx| {
                        if card.epoch != epoch {
                            return false;
                        }
                        if let Some(dialog) = &card.catalog {
                            dialog.update(cx, |dialog, cx| dialog.set_icon(id, svg, cx));
                        }
                        true
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
        }));
        cx.notify();
    }
    /// Review fixture: one saved agent; `dialog` opens Add ACP agent over a
    /// synthetic registry.
    #[cfg(feature = "browser-fixture")]
    pub fn fixture_seed(&mut self, dialog: bool, cx: &mut Context<Self>) {
        self.fixture = true;
        let agent = |id: &str, name: &str, description: &str, package: &str| RegistryAgent {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            icon: None,
            distribution: serde_json::from_value(
                json!({"npx": {"package": package, "args": ["--acp"]}}),
            )
            .unwrap_or_default(),
            ..Default::default()
        };
        let mut registry = vec![
            agent(
                "fieldnotes-acp",
                "Fieldnotes Agent",
                "Notes-first coding agent for the Fieldnotes workspace",
                "@fieldnotes/agent",
            ),
            agent(
                "harbor",
                "Harbor CLI",
                "Terminal agent that plans, edits and runs tests",
                "harbor-cli",
            ),
            agent(
                "lattice",
                "Lattice Code",
                "Refactoring assistant for large monorepos",
                "@lattice/code",
            ),
            agent(
                "northwind",
                "Northwind Agent",
                "Agent for data pipelines and notebooks",
                "northwind-agent",
            ),
            agent(
                "quill",
                "Quill",
                "Writes and reviews documentation alongside code",
                "quill-acp",
            ),
        ];
        registry.extend(acp_registry::fallback());
        registry.sort_by_key(|agent| agent.name.to_lowercase());
        let saved = registry
            .iter()
            .find(|agent| agent.id == "fieldnotes-acp")
            .and_then(|agent| acp_registry::offer(agent, "linux-x86_64"));
        let mut agents = Vec::new();
        if let Some(Offer::Add(saved)) = saved {
            agents.push(saved);
        }
        self.fixture_catalog = Some(Catalog {
            platform: acp_registry::platform(),
            agents: registry,
            error: None,
        });
        self.runtime = agents
            .iter()
            .map(|agent| json!({"id":agent.id,"enabled":true,"installed":true}))
            .collect();
        self.snapshot = Some(CustomAcpAgents {
            revision: 1,
            agents,
        });
        self.fixture_open = dialog;
        cx.notify();
    }
}
impl Render for CustomAcpCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.row_events.is_none() {
            self.row_events = Some(cx.subscribe_in(
                &self.rows,
                window,
                |this, _, event: &RowEvent, window, cx| {
                    if event.scope == this.epoch {
                        this.row_action(&event.action, window, cx);
                    }
                },
            ));
        }
        if self.fixture_open {
            self.fixture_open = false;
            self.open_dialog(window, cx);
        }
        let data = RowData {
            scope: self.epoch,
            agents: self
                .snapshot
                .as_ref()
                .map(|s| s.agents.clone())
                .unwrap_or_default(),
            loaded: self.snapshot.is_some(),
            advanced: true,
            runtime: self.runtime.clone(),
            blocked: self.busy || (!self.fixture && self.state.read(cx).engine().is_none()),
            busy: self.busy_label.clone(),
            error: if self.form.is_none() && !self.dialog_open {
                self.error.clone()
            } else {
                None
            },
            probes: self.probes.clone(),
        };
        self.rows.update(cx, |rows, cx| rows.sync(data, cx));
        div()
            .child(self.rows.clone())
            .children(self.dialog_open.then(|| self.catalog.clone()).flatten())
            .children(self.form.clone())
    }
}
