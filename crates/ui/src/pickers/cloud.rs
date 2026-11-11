//! Prepared destinations extend the native device picker and keep local picks intact.
use super::*;
use serde_json::{Value, json};

mod connections;
mod providers;

#[derive(Clone)]
pub(crate) struct CloudCanvasChoices {
    draft: DraftConfig,
    run: ResolvedRunConfig,
    source: zuse_viewport::creation_sources::SelectionSnapshot,
}
#[derive(Default)]
struct ProviderPreferences(HashMap<(String, Option<String>), String>);
impl gpui::Global for ProviderPreferences {}
/// The last cloud context each account and organization received, so a new
/// composer (every opened cloud chat has one) shows its models at once.
#[derive(Default)]
struct CloudContextCache(HashMap<(String, Option<String>), Value>);
impl gpui::Global for CloudContextCache {}
use connections::ConnectionChoices;

#[derive(Default)]
pub(super) struct Cloud {
    pub(super) frozen: bool,
    provider_preference: Option<String>,
    providers_expanded: bool,
    pub(super) task_binding: Option<TaskBinding>,
    owner: Option<String>,
    organization: Option<String>,
    context: Value,
    loaded: bool,
    task: Option<Task<()>>,
    rows: Vec<Value>,
    error: Option<String>,
    local_checkout: Option<(Option<String>, CheckoutKind)>,
    connection_choices: std::cell::RefCell<ConnectionChoices>,
    pub(super) catalog: Option<zuse_protocol::cloud_catalog::CloudAgentCatalog>,
    pub(super) harnesses: Loadable<Vec<HarnessDescriptor>>,
    pub(super) models: HashMap<HarnessId, Loadable<Vec<Model>>>,
    pub(super) model_error: Option<String>,
    pub(super) model_waiting: bool,
}
pub(super) struct TaskBinding {
    pub(super) target: crate::state::CloudTarget,
    pub(super) provider: String,
    pub(super) choice: Option<ChatConfig>,
}
impl Cloud {
    pub(super) fn model_read_pending(&self) -> bool {
        self.task.is_some()
    }

    pub(super) fn set_context_error(&mut self, error: String) {
        self.error = Some(error);
    }

    #[cfg(test)]
    pub(super) fn set_project_rows_for_test(&mut self, rows: Vec<Value>) {
        self.rows = rows;
    }

    pub(super) fn refresh(&mut self) {
        self.loaded = false;
    }
}
impl Pickers {
    #[cfg(test)]
    pub(crate) fn seed_cloud_creation_for_test(
        &mut self,
        context: Value,
        project: Value,
        cx: &mut Context<Self>,
    ) {
        self.cloud.owner = context["accountId"].as_str().map(str::to_owned);
        self.cloud.organization = context["organizationId"].as_str().map(str::to_owned);
        self.cloud.loaded = true;
        self.cloud.context = context;
        self.cloud.rows = vec![project.clone()];
        self.select_cloud(project, cx);
    }
    #[cfg(test)]
    pub(crate) fn seed_cloud_source_for_test(
        &mut self,
        source: zuse_protocol::creation_source::Resolution,
        cx: &mut Context<Self>,
    ) {
        self.source_picker.update(cx, |picker, cx| {
            picker.restore_selection(
                zuse_viewport::creation_sources::SelectionSnapshot::new(None, Some(source)),
                cx,
            )
        });
    }
    pub(crate) fn revoke_cloud_canvas_scope(&mut self, cx: &mut Context<Self>) {
        self.source_tasks.clear();
        self.source_picker.update(cx, |picker, cx| {
            picker.restore_selection(
                zuse_viewport::creation_sources::SelectionSnapshot::new(None, None),
                cx,
            )
        });
        self.cloud = Cloud::default();
        self.catalog_rev += 1;
        cx.notify();
    }
    pub(crate) fn cloud_canvas_choices(&self, cx: &App) -> CloudCanvasChoices {
        CloudCanvasChoices {
            draft: self.config.clone(),
            run: self.resolved(cx),
            source: self.source_picker.read(cx).selection_snapshot(),
        }
    }
    pub(crate) fn start_cloud_canvas(&mut self, project: Option<Value>, cx: &mut Context<Self>) {
        self.source_picker
            .update(cx, |picker, cx| picker.clear_selection(cx));
        self.config.branch = None;
        if let Some(project) = project {
            self.select_cloud(project, cx);
        }
    }
    pub(crate) fn restore_cloud_canvas(
        &mut self,
        project: Option<Value>,
        choice: Option<CloudCanvasChoices>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud.task_binding.is_some() {
            return;
        }
        let expected = self
            .state
            .read(cx)
            .cloud_organization_id()
            .map(|id| format!("organization:{id}"))
            .or_else(|| self.state.read(cx).cloud_user_id().map(str::to_owned));
        let project = project.filter(|row| row["accountId"].as_str() == expected.as_deref());
        self.cloud.provider_preference = project
            .as_ref()
            .and_then(|row| row["providerId"].as_str())
            .map(str::to_owned);
        self.state.update(cx, |state, cx| {
            state.cloud_project = project.clone();
            cx.notify();
        });
        if let Some(provider) = self.cloud.provider_preference.clone() {
            self.cloud.catalog = None;
            self.cloud.models.clear();
            self.cloud.harnesses = Loadable::Idle;
            self.receive_cloud_models(
                self.cloud.context["providerAgentCatalogs"][provider].clone(),
                cx,
            );
        }
        if let Some(saved) = choice {
            self.config = saved.draft;
            self.source_picker
                .update(cx, |picker, cx| picker.restore_selection(saved.source, cx));
            let choice = saved.run;
            self.config.harness = choice.harness;
            self.config.model = choice.model.clone();
            self.config.reasoning = choice.reasoning;
            self.config.sandbox = Some(choice.sandbox);
            if let (Some(agent), Some(model)) = (choice.harness, choice.model) {
                *self.defaults.model_options_mut(agent, &model) = choice.model_options;
            }
        }
        self.catalog_rev += 1;
        cx.notify();
    }
    /// Account metadata and pending run intent never become an execution engine.
    pub(crate) fn bind_cloud_task(
        &mut self,
        target: crate::state::CloudTarget,
        provider: String,
        seed: Option<ChatConfig>,
        cx: &mut Context<Self>,
    ) {
        let same = self.cloud.task_binding.as_ref().is_some_and(|binding| {
            binding.target.subject == target.subject
                && binding.target.organization == target.organization
                && binding.target.workspace == target.workspace
                && binding.target.initial_session == target.initial_session
        });
        if !same {
            self.cloud.model_waiting = false;
            self.cloud.task = None;
            self.cloud.loaded = false;
            self.cloud.context = Value::Null;
            self.cloud.task_binding = Some(TaskBinding {
                target,
                provider,
                choice: None,
            });
        }
        if self
            .cloud
            .task_binding
            .as_ref()
            .is_some_and(|binding| binding.choice.is_none())
        {
            if let Some(seed) = seed.filter(|seed| {
                seed.model
                    .as_deref()
                    .is_some_and(|model| !model.trim().is_empty())
            }) {
                self.set_cloud_task_choice(seed, cx);
            }
        }
    }
    /// Deterministic focus/render tests use already-loaded, scoped metadata.
    /// This is never linked into a production build and grants no execution.
    #[cfg(test)]
    pub(crate) fn seed_cloud_task_catalog_for_test(&mut self, cx: &mut Context<Self>) {
        let binding = self
            .cloud
            .task_binding
            .as_ref()
            .expect("task authority must already be bound");
        let choice = binding
            .choice
            .as_ref()
            .expect("test task needs saved agent settings");
        let model = choice.model.clone().expect("test task needs a saved model");
        self.cloud.owner = Some(binding.target.subject.clone());
        self.cloud.organization = binding.target.organization.clone();
        self.cloud.loaded = true;
        self.cloud.context = json!({"accountId":binding.target.subject,"organizationId":binding.target.organization});
        let catalog = zuse_protocol::cloud_catalog::CloudAgentCatalog {
            providers: vec![zuse_protocol::cloud_catalog::CloudAgentModels {
                id: choice.harness,
                name: "Fixture agent".into(),
                default_model: model.clone(),
                models: vec![Model {
                    id: model.clone(),
                    label: model,
                    description: None,
                    reasoning_levels: vec![],
                    options: vec![],
                }],
            }],
        };
        self.receive_cloud_models(serde_json::to_value(catalog).unwrap(), cx);
    }
    pub(crate) fn clear_cloud_task_binding(&mut self, cx: &mut Context<Self>) {
        self.cloud.task_binding = None;
        self.cloud.task = None;
        self.cloud.context = Value::Null;
        self.cloud.models.clear();
        self.cloud.catalog = None;
        self.cloud.harnesses = Loadable::Idle;
        self.cloud.model_waiting = false;
        self.cloud.loaded = false;
        cx.notify();
    }
    pub(crate) fn cloud_task_choice(&self) -> Option<ResolvedRunConfig> {
        let choice = self.cloud.task_binding.as_ref()?.choice.as_ref()?;
        Some(ResolvedRunConfig {
            harness: Some(choice.harness),
            model: choice.model.clone(),
            reasoning: choice.reasoning,
            model_options: choice.model_options.clone(),
            sandbox: choice.sandbox,
            plan: false,
        })
    }
    /// Change a bound cloud task's access mode; its next message carries it.
    pub(crate) fn set_cloud_task_sandbox(
        &mut self,
        level: SandboxLevel,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut choice) = self
            .cloud
            .task_binding
            .as_ref()
            .and_then(|binding| binding.choice.clone())
        else {
            return false;
        };
        choice.sandbox = level;
        self.set_cloud_task_choice(choice, cx);
        true
    }
    pub(super) fn set_cloud_task_choice(&mut self, choice: ChatConfig, cx: &mut Context<Self>) {
        let Some(binding) = &mut self.cloud.task_binding else {
            return;
        };
        self.config.harness = Some(choice.harness);
        self.config.model = choice.model.clone();
        self.config.reasoning = choice.reasoning;
        self.config.sandbox = Some(choice.sandbox);
        binding.choice = Some(choice);
        self.catalog_rev += 1;
        cx.notify();
    }

    pub(crate) fn start_without_project(&mut self, cx: &mut Context<Self>) {
        self.pick_no_project(cx);
    }
    pub(super) fn leave_cloud(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _| state.cloud_project = None);
        if let Some((branch, checkout)) = self.cloud.local_checkout.take() {
            self.config.branch = branch;
            self.config.checkout = checkout;
        }
        self.catalog_rev += 1;
    }
    fn select_cloud(&mut self, mut row: Value, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let expected = state
            .cloud_organization_id()
            .map(|org| format!("organization:{org}"))
            .or_else(|| state.cloud_user_id().map(str::to_owned));
        if row["accountId"].as_str() != expected.as_deref() {
            self.cloud.error =
                Some("Your account or organization changed. Open the picker again.".into());
            cx.notify();
            return;
        }
        if self.cloud.task_binding.is_some() || self.cloud_launch_frozen() {
            return;
        }
        let preferred = self.cloud.owner.as_ref().and_then(|owner| {
            cx.default_global::<ProviderPreferences>()
                .0
                .get(&(owner.clone(), self.cloud.organization.clone()))
                .cloned()
        });
        self.cloud.provider_preference = preferred;
        match providers::select(
            &self.cloud.context,
            self.cloud.provider_preference.as_deref(),
            row["providerId"].as_str(),
        ) {
            Ok(provider) => row["providerId"] = json!(provider),
            Err(_) => {
                row["providerId"] = self
                    .cloud
                    .provider_preference
                    .as_ref()
                    .map(|id| json!(id))
                    .unwrap_or(Value::Null)
            }
        }
        let provider = row["providerId"].as_str().unwrap_or("");
        if self.cloud.context["providerAgentCatalogs"].is_object() {
            self.cloud.catalog = None;
            self.cloud.models.clear();
            self.cloud.harnesses = Loadable::Idle;
            self.receive_cloud_models(
                self.cloud.context["providerAgentCatalogs"][provider].clone(),
                cx,
            );
        }
        self.cloud
            .local_checkout
            .get_or_insert_with(|| (self.config.branch.clone(), self.config.checkout));
        self.align_cloud_model(cx);
        self.config.branch = None;
        self.catalog_rev += 1;
        self.state.update(cx, |state, cx| {
            state.cloud_project = Some(row);
            cx.notify();
        });
        self.close(cx);
    }
    fn align_cloud_model(&mut self, cx: &mut Context<Self>) {
        let current = self.resolved(cx);
        if let Some(provider) = self.cloud.catalog.as_ref().and_then(|catalog| {
            catalog
                .providers
                .iter()
                .find(|provider| Some(provider.id) == current.harness)
                .or_else(|| catalog.providers.first())
        }) {
            let same = current
                .model
                .as_deref()
                .filter(|_| current.harness == Some(provider.id))
                .and_then(|model| provider.resolve_model(model));
            let model = same
                .or_else(|| {
                    crate::settings::default_model(cx)
                        .filter(|model| model.provider == provider.id)
                        .and_then(|model| provider.resolve_model(&model.model))
                })
                .or_else(|| {
                    self.defaults
                        .model_for(provider.id)
                        .and_then(|model| provider.resolve_model(&model.id))
                })
                .map(|model| model.id.clone())
                .unwrap_or_else(|| provider.default_model.clone());
            self.config.reasoning = if same.is_some() {
                current.reasoning
            } else {
                None
            };
            if same.is_some() {
                *self.defaults.model_options_mut(provider.id, &model) = current.model_options;
            }
            self.config.harness = Some(provider.id);
            self.config.model = Some(model);
        } else {
            self.config.harness = None;
            self.config.model = None;
            self.config.reasoning = None;
        }
    }
    pub(super) fn ensure_cloud(&mut self, cx: &mut Context<Self>) {
        if !crate::cloud_beta() {
            return;
        }
        let state = self.state.read(cx);
        let owner = self
            .cloud
            .task_binding
            .as_ref()
            .map(|binding| binding.target.subject.clone())
            .or_else(|| state.cloud_user_id().map(str::to_owned));
        let organization = self
            .cloud
            .task_binding
            .as_ref()
            .map(|binding| binding.target.organization.clone())
            .unwrap_or_else(|| state.cloud_organization_id().map(str::to_owned));
        let engine = self
            .cloud
            .task_binding
            .as_ref()
            .map(|binding| binding.target.controller.clone())
            .or_else(|| state.engine().map(|engine| engine.controller()));
        if self.cloud.owner != owner || self.cloud.organization != organization {
            self.cloud.frozen = false;
            self.cloud.provider_preference = owner.as_ref().and_then(|id| {
                cx.default_global::<ProviderPreferences>()
                    .0
                    .get(&(id.clone(), organization.clone()))
                    .cloned()
            });
            self.cloud.providers_expanded = false;
            self.cloud.owner = owner.clone();
            self.cloud.organization = organization.clone();
            self.cloud.context = Value::Null;
            self.cloud.loaded = false;
            self.cloud.rows.clear();
            self.cloud.task = None;
            self.cloud.models.clear();
            self.cloud.harnesses = Loadable::Idle;
            self.cloud.catalog = None;
            self.cloud.model_error = None;
            self.cloud.model_waiting = false;
            self.cloud.error = None;
            *self.cloud.connection_choices.borrow_mut() = ConnectionChoices::default();
            self.catalog_rev += 1;
        }
        if self.cloud.loaded || self.cloud.task.is_some() {
            return;
        }
        let Some(owner) = owner else {
            return;
        };
        let Some(engine) = engine else {
            return;
        };
        self.cloud.loaded = true;
        // The last context this account saw paints immediately; the fresh
        // read below replaces it.
        let cached = cx
            .default_global::<CloudContextCache>()
            .0
            .get(&(owner.clone(), organization.clone()))
            .cloned();
        let had_cache = cached.is_some();
        if let Some(value) = cached {
            self.apply_cloud_context(value, &owner, cx);
        }
        let request_owner = owner.clone();
        let request_organization = organization.clone();
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(methods::CLOUD_CONTEXT, json!({"expectedAccountId":request_owner,"expectedOrganizationId":request_organization}))
                .await
        });
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let current=this.cloud.task_binding.as_ref().map(|binding|(Some(binding.target.subject.as_str()),binding.target.organization.as_deref()))
                    .unwrap_or_else(||(this.state.read(cx).cloud_user_id(),this.state.read(cx).cloud_organization_id()));
                if current!=(Some(owner.as_str()),organization.as_deref()){return;}
                this.cloud.task = None;
                match result {
                    Ok(Ok(value))
                        if value["accountId"].as_str() == Some(owner.as_str()) && value["organizationId"].as_str()==organization.as_deref() =>
                    {
                        cx.default_global::<CloudContextCache>()
                            .0
                            .insert((owner.clone(), organization.clone()), value.clone());
                        this.apply_cloud_context(value, &owner, cx);
                    }
                    // A cached catalog is already on screen; a failed refresh
                    // keeps it rather than replacing it with an error.
                    Ok(Err(_)) | Err(_) if had_cache => {}
                    result => {
                        let error = match result {
                            Ok(Err(error)) => error,
                            _ => {
                                zuse_rpc::RpcError::Failed("Could not load Cloud destinations. Open the picker again to retry.".into())
                            }
                        };
                        this.receive_cloud_model_error(error, cx);
                    }
                }
                cx.notify();
            });
        }));
    }
    /// Show a cloud context: its destinations and the selected provider's
    /// models. Fresh replies and the cached copy shown before them both land
    /// here.
    fn apply_cloud_context(&mut self, value: Value, owner: &str, cx: &mut Context<Self>) {
        self.cloud.context = value.clone();
        let provider = self
            .cloud
            .task_binding
            .as_ref()
            .map(|binding| binding.provider.clone())
            .unwrap_or_else(|| {
                let project = self.state.read(cx).cloud_project.clone();
                providers::select(
                    &value,
                    self.cloud.provider_preference.as_deref(),
                    project.as_ref().and_then(|p| p["providerId"].as_str()),
                )
                .unwrap_or_else(|_| self.cloud.provider_preference.clone().unwrap_or_default())
            });
        if self.cloud.task_binding.is_none() {
            self.state.update(cx, |state, _| {
                if let Some(project) = &mut state.cloud_project {
                    project["providerId"] = json!(provider);
                }
            });
        }
        let catalog = if value["providerAgentCatalogs"].is_object() {
            value["providerAgentCatalogs"][provider].clone()
        } else {
            value["agentCatalog"].clone()
        };
        if self.cloud.task_binding.is_none() {
            self.cloud.catalog = None;
            self.cloud.models.clear();
            self.cloud.harnesses = Loadable::Idle;
        }
        self.receive_cloud_models(catalog, cx);
        if self.cloud.task_binding.is_none() {
            self.align_cloud_model(cx);
        }
        self.cloud.rows = value["projects"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut row| {
                row["accountId"] = value["ownerId"]
                    .as_str()
                    .map(|s| json!(s))
                    .unwrap_or_else(|| json!(owner));
                if row["providerId"].is_null() || row["providerId"] == "box" {
                    row["providerId"] = json!("boxd");
                }
                row
            })
            .collect();
        self.cloud.error = None;
    }
    /// Cloud status only matters once the account has cloud destinations; a
    /// failed or signed-out lookup with nothing to offer stays out of the
    /// computer picker.
    fn cloud_notice(&self) -> Option<String> {
        (!self.cloud.rows.is_empty())
            .then(|| self.cloud.error.clone())
            .flatten()
    }
    pub(super) fn filtered_cloud_projects(&self, cx: &App) -> Vec<Value> {
        let query = self.search.read(cx).text().to_lowercase();
        self.cloud
            .rows
            .iter()
            .filter(|p| p["canCreate"] != false)
            .filter(|p| {
                format!(
                    "{} {}",
                    p["displayName"].as_str().unwrap_or(""),
                    p["repositoryIdentity"].as_str().unwrap_or("")
                )
                .to_lowercase()
                .contains(&query)
            })
            .cloned()
            .collect()
    }
    pub(crate) fn cloud_connection_for(&self, agent: HarnessId) -> Option<String> {
        self.cloud.connection_choices.borrow_mut().resolve(
            &self.cloud.context,
            self.cloud.owner.as_deref()?,
            self.cloud.organization.as_deref(),
            agent,
        )
    }
    pub(crate) fn cloud_launch_provider(&self, cx: &App) -> Result<String, String> {
        if let Some(binding) = &self.cloud.task_binding {
            return Ok(binding.provider.clone());
        }
        providers::select(
            &self.cloud.context,
            self.cloud.provider_preference.as_deref(),
            self.state
                .read(cx)
                .cloud_project
                .as_ref()
                .and_then(|p| p["providerId"].as_str()),
        )
    }
    pub(crate) fn cloud_launch_size(&self, provider: &str, cx: &App) -> Result<String, String> {
        let state = self.state.read(cx);
        let preferred = state
            .cloud_project
            .as_ref()
            .filter(|project| project["providerId"] == provider)
            .and_then(|project| project["sizeId"].as_str());
        providers::size(&self.cloud.context, provider, preferred)
    }
    fn choose_cloud_provider(&mut self, id: String, cx: &mut Context<Self>) {
        if self.cloud.task_binding.is_some()
            || self.cloud_launch_frozen()
            || !providers::available(&self.cloud.context)
                .iter()
                .any(|row| row.id == id)
        {
            return;
        }
        let Some(owner) = self.cloud.owner.clone() else {
            return;
        };
        cx.default_global::<ProviderPreferences>()
            .0
            .insert((owner, self.cloud.organization.clone()), id.clone());
        self.cloud.provider_preference = Some(id.clone());
        if let Some(mut project) = self.cloud_destination(cx) {
            project["providerId"] = json!(id);
            self.select_cloud(project, cx);
        }
        cx.notify();
    }
    fn cloud_computer_rows(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let locked = self.cloud.task_binding.is_some();
        let provider = self
            .cloud_launch_provider(cx)
            .ok()
            .or_else(|| self.cloud.provider_preference.clone());
        let mut view = div().flex().flex_col().child(
            zuse_viewport::creation_style::option(
                "cloud-computer-options",
                format!("Computer · {}", provider.as_deref().unwrap_or("Choose")),
                "monitor",
                false,
            )
            .debug_selector(|| "cloud-computer-options".into())
            .child(zuse_viewport::creation_style::meta(if locked {
                "Task computer"
            } else if self.cloud.providers_expanded {
                "▾"
            } else {
                "▸"
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                if this.cloud.task_binding.is_none() {
                    this.cloud.providers_expanded = !this.cloud.providers_expanded;
                    cx.notify();
                }
            })),
        );
        if !locked && self.cloud.providers_expanded {
            for (i, row) in providers::available(&self.cloud.context)
                .into_iter()
                .enumerate()
            {
                let chosen = provider.as_deref() == Some(row.id.as_str());
                let id = row.id.clone();
                let debug_id = format!("cloud-computer-provider-{id}");
                view =
                    view.child(
                        zuse_viewport::creation_style::option(
                            ("cloud-computer-provider", i),
                            row.id.to_uppercase(),
                            "cloud-dither",
                            chosen,
                        )
                        .debug_selector(move || debug_id.clone())
                        .child(zuse_viewport::creation_style::selection(chosen))
                        .when_some(row.reason, |view, reason| {
                            view.child(zuse_viewport::creation_style::meta(reason))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose_cloud_provider(id.clone(), cx)
                        })),
                    );
            }
        }
        if !locked {
            if let Err(reason) = self.cloud_launch_provider(cx) {
                view = view.child(
                    div()
                        .p_2()
                        .text_size(px(11.))
                        .text_color(Theme::of(cx).warning)
                        .child(reason),
                );
            }
        }
        view.into_any_element()
    }
    pub(crate) fn cloud_v2_available(&self, provider: &str) -> bool {
        self.cloud.context["commandAdmissionVersion"] == 2
            && self.cloud.context["computerCapabilities"]["providers"]
                .as_array()
                .is_some_and(|rows| {
                    rows.iter()
                        .any(|p| p["id"] == provider && p["commandAdmissionVersion"] == 2)
                })
    }
    pub(super) fn cloud_destination(&self, cx: &App) -> Option<Value> {
        let state = self.state.read(cx);
        if let Some(project) = &state.cloud_project {
            return Some(project.clone());
        }
        let identity = state.selected_space_row()?.remote_repository.as_ref()?;
        self.cloud
            .rows
            .iter()
            .find(|p| {
                p["repositoryIdentity"]
                    .as_str()
                    .is_some_and(|v| v.eq_ignore_ascii_case(identity))
            })
            .cloned()
    }
    pub(super) fn cloud_project_rows(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_cloud(cx);
        let selected = self
            .state
            .read(cx)
            .cloud_project
            .as_ref()
            .map(|p| p["projectId"].clone());
        let base = self.filtered_project_rows(cx).len() + 1;
        let mut view = div().flex().flex_col();
        for (i, project) in self.filtered_cloud_projects(cx).into_iter().enumerate() {
            let chosen = selected.as_ref() == Some(&project["projectId"]);
            view = view.child(
                zuse_viewport::creation_style::option(
                    ("cloud-project", i),
                    project["displayName"]
                        .as_str()
                        .unwrap_or("Repository")
                        .to_owned(),
                    "folder",
                    chosen,
                )
                .h(px(32.))
                .when(self.active == base + i, |v| {
                    v.bg(Theme::of(cx).element_hover)
                })
                .child(zuse_viewport::creation_style::selection(chosen))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_cloud(project.clone(), cx);
                })),
            );
        }
        // Computer/model readiness is unrelated to selecting a repository.
        // Cloud setup errors belong to the computer picker, not local projects.
        view.into_any_element()
    }
    pub(super) fn select_cloud_index(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.filtered_cloud_projects(cx).get(i).cloned() {
            self.select_cloud(row, cx);
        }
    }
    pub(super) fn select_cloud_destination(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.cloud_destination(cx) {
            self.select_cloud(row, cx);
        }
    }
    pub(super) fn cloud_rows(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_cloud(cx);
        let theme = Theme::of(cx).for_popup();
        let selected = self
            .state
            .read(cx)
            .cloud_project
            .as_ref()
            .map(|p| p["projectId"].clone());
        let destination = self.cloud_destination(cx);
        let ready = self.cloud_launch_provider(cx).is_ok();
        let computer_rows = if (destination.is_some() || self.cloud.task_binding.is_some()) && ready
        {
            self.cloud_computer_rows(cx)
        } else {
            div().into_any_element()
        };
        let selected_provider = self
            .cloud_launch_provider(cx)
            .ok()
            .or_else(|| self.cloud.provider_preference.clone())
            .unwrap_or_else(|| "Choose computer".into());
        div()
            .flex()
            .flex_col()
            .child(computer_rows)
            .when_some(destination.filter(|_| ready), |view, row| {
                let selected = selected.as_ref() == Some(&row["projectId"]);
                view.child(zuse_viewport::creation_style::separator())
                    .child(
                        zuse_viewport::creation_style::option(
                            "cloud-destination",
                            format!("Cloud · {selected_provider}"),
                            "cloud-dither",
                            selected,
                        )
                        .when(self.active == self.filtered_device_rows(cx).len(), |v| {
                            v.bg(theme.element_hover)
                        })
                        .child(zuse_viewport::creation_style::meta("Beta"))
                        .child(zuse_viewport::creation_style::selection(selected))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_cloud(row.clone(), cx)),
                        ),
                    )
            })
            .child(zuse_viewport::creation_style::separator())
            .child(
                zuse_viewport::creation_style::option(
                    "add-computer",
                    if self.cloud_destination(cx).is_none() {
                        "Add project to Cloud…"
                    } else {
                        "Cloud setup…"
                    },
                    "cloud-dither",
                    false,
                )
                .debug_selector(|| "connect-project-to-cloud".into())
                .role(gpui::Role::Button)
                .on_click(cx.listener(|this, _, window, cx| {
                    let target = {
                        let state = this.state.read(cx);
                        state
                            .engine()
                            .map(|engine| engine.controller())
                            .zip(state.cloud_user_id().map(str::to_owned))
                            .map(|(engine, user)| {
                                (
                                    engine,
                                    user,
                                    state.cloud_organization_id().map(str::to_owned),
                                )
                            })
                    };
                    this.cloud.refresh();
                    this.close(cx);
                    if let Some((controller, subject, organization)) = target {
                        crate::cloud_history::open_setup(controller, subject, organization, cx);
                    } else {
                        window.dispatch_action(Box::new(crate::shell::OpenDevices), cx);
                    }
                })),
            )
            .when_some(self.cloud_notice(), |view, error| {
                view.child(
                    div()
                        .p_2()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(SharedString::from(error)),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod project_selection_tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    fn provider_context() -> Value {
        let mut catalogs = serde_json::Map::new();
        let mut caps = Vec::new();
        for id in ["boxd", "boat", "e2b", "daytona"] {
            catalogs.insert(id.into(),json!({"providers":[{"id":"codex","name":"Codex","defaultModel":format!("{id}-model"),"models":[{"id":format!("{id}-model"),"label":id}]}]}));
            caps.push(json!({"id":id,"configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":id!="daytona","supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],"unavailableReason":null}));
        }
        json!({"accountId":"fixture-user","ownerId":"fixture-user","organizationId":null,"commandAdmissionVersion":2,"computerCapabilities":{"providers":caps},"providerAgentCatalogs":catalogs})
    }
    struct ComputerRows(Entity<Pickers>);
    impl Render for ComputerRows {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(600.)).h(px(300.)).child(
                self.0
                    .update(cx, |pickers, cx| pickers.cloud_computer_rows(cx)),
            )
        }
    }
    struct UnlinkedComputerRows(Entity<Pickers>);
    impl Render for UnlinkedComputerRows {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(600.))
                .h(px(400.))
                .child(self.0.update(cx, |p, cx| p.cloud_rows(cx)))
        }
    }
    #[gpui::test]
    fn unlinked_project_offers_connect_instead_of_unusable_cloud_computer(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut s = AppState::new();
                s.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
                s
            });
            UnlinkedComputerRows(cx.new(|cx| {
                let mut p = Pickers::new(state, cx);
                p.cloud.owner = Some("fixture-user".into());
                p.cloud.loaded = true;
                p.cloud.context = provider_context();
                p
            }))
        });
        cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            assert!(
                visual.debug_bounds("cloud-computer-options").is_none(),
                "an unlinked local project cannot select a cloud computer"
            );
            assert!(
                visual.debug_bounds("connect-project-to-cloud").is_some(),
                "explicit connection action is available"
            );
        }
        window.update(cx, |_, w, _| w.remove_window()).unwrap();
        cx.run_until_parked();
    }
    #[gpui::test]
    fn unavailable_cloud_provider_offers_setup_instead_of_choose_computer(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let window=cx.add_window(|_,cx| {
            let state=cx.new(|_| {let mut s=AppState::new();s.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));s});
            UnlinkedComputerRows(cx.new(|cx| {
                let mut p=Pickers::new(state,cx);
                p.seed_cloud_creation_for_test(provider_context(),json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","repositoryIdentity":"github.com/fixture/repo"}),cx);
                for row in p.cloud.context["computerCapabilities"]["providers"].as_array_mut().unwrap(){row["managedBaseReady"]=json!(false);}
                p
            }))
        });
        cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            assert!(visual.debug_bounds("cloud-computer-options").is_none());
            assert!(visual.debug_bounds("connect-project-to-cloud").is_some());
        }
        window.update(cx, |_, w, _| w.remove_window()).unwrap();
        cx.run_until_parked();
    }
    #[gpui::test]
    fn clicking_computer_provider_keeps_repository_and_loads_its_models(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let handle=cx.add_window(|_,cx| {
            let state=cx.new(|_| {let mut state=AppState::new();state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));state});
            ComputerRows(cx.new(|cx| {let mut pickers=Pickers::new(state,cx);pickers.seed_cloud_creation_for_test(provider_context(),json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","repositoryIdentity":"github.com/fixture/repo"}),cx);pickers.cloud.providers_expanded=true;pickers}))
        });
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
            let row = visual
                .debug_bounds("cloud-computer-provider-e2b")
                .expect("actual E2B provider row painted");
            visual.simulate_click(row.center(), gpui::Modifiers::default());
        }
        cx.run_until_parked();
        handle
            .read_with(cx, |host, cx| {
                let p = host.0.read(cx);
                assert_eq!(
                    p.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                    "repository"
                );
                assert_eq!(p.cloud_launch_provider(cx).unwrap(), "e2b");
                assert_eq!(p.resolved(cx).model.as_deref(), Some("e2b-model"));
                assert_eq!(p.cloud.provider_preference.as_deref(), Some("e2b"));
            })
            .unwrap();
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
    }
    #[gpui::test]
    fn bound_task_locks_provider_and_preserves_sleeping_model_choice(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut s = AppState::new();
            s.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            s
        });
        let p = cx.new(|cx| Pickers::new(state.clone(), cx));
        p.update(cx, |p, cx| {
            p.seed_cloud_creation_for_test(
                provider_context(),
                json!({"projectId":"repository","accountId":"fixture-user","providerId":"e2b"}),
                cx,
            );
            let target = crate::state::CloudTarget {
                controller,
                subject: "fixture-user".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            };
            let choice = ChatConfig {
                harness: HarnessId::Codex,
                model: Some("retained-model".into()),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
                plan: false,
            };
            p.bind_cloud_task(target.clone(), "e2b".into(), Some(choice.clone()), cx);
            p.choose_cloud_provider("boat".into(), cx);
            state.update(cx, |state, _| {
                state.connection = crate::state::ConnectionStatus::Failed("Sleeping".into())
            });
            p.bind_cloud_task(target, "boat".into(), Some(choice), cx);
            assert_eq!(p.cloud_launch_provider(cx).unwrap(), "e2b");
            assert_eq!(
                p.cloud_task_choice().unwrap().model.as_deref(),
                Some("retained-model")
            );
            assert!(matches!(
                state.read(cx).connection,
                crate::state::ConnectionStatus::Failed(_)
            ));
        });
        drop(receive);
    }
    /// Render the production project rows, retaining their real click listeners.
    /// No keyboard helper or direct selection call substitutes for the click.
    struct ProjectRows(Entity<Pickers>);

    impl Render for ProjectRows {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("pending-cloud-project-row-fixture")
                .debug_selector(|| "pending-cloud-project-row-fixture".into())
                .w(px(400.))
                .h(px(32.))
                .child(
                    self.0
                        .update(cx, |pickers, cx| pickers.cloud_project_rows(cx)),
                )
        }
    }

    #[gpui::test]
    fn task_model_changes_before_ready_survive_history_rebinding_and_sleep(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.selected_chat = Some("session".into());
            state
        });
        let pickers = cx.new(|cx| Pickers::new(state.clone(), cx));
        pickers.update(cx, |pickers, cx| {
            let target = crate::state::CloudTarget {
                controller,
                subject: "owner".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            };
            let seed = ChatConfig {
                harness: HarnessId::Codex,
                model: Some("original".into()),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
                plan: false,
            };
            pickers.bind_cloud_task(target.clone(), "boxd".into(), Some(seed.clone()), cx);
            // Retain an already-authorized account metadata cache. No guest or
            // catalogue RPC participates in changing this pending command intent.
            pickers.cloud.owner = Some("owner".into());
            pickers.cloud.loaded = true;
            pickers.pick_harness(HarnessId::ClaudeCode, cx);
            pickers.pick_model("explicit-model".into(), cx);
            pickers.pick_reasoning(ReasoningLevel::High, cx);
            pickers.pick_option("mode".into(), "fast".into(), false, cx);
            pickers.bind_cloud_task(target.clone(), "boxd".into(), Some(seed.clone()), cx);
            let choice = pickers.cloud_task_choice().unwrap();
            assert_eq!(choice.harness, Some(HarnessId::ClaudeCode));
            assert_eq!(choice.model.as_deref(), Some("explicit-model"));
            assert_eq!(choice.reasoning, Some(ReasoningLevel::High));
            assert_eq!(choice.model_options["mode"], "fast");
            assert!(state.read(cx).engine().is_none());
            state.update(cx, |state, _| {
                state.connection = crate::state::ConnectionStatus::Failed("Sleeping".into())
            });
            pickers.bind_cloud_task(target, "boxd".into(), Some(seed), cx);
            assert_eq!(
                pickers.cloud_task_choice().unwrap().model.as_deref(),
                Some("explicit-model")
            );
            pickers.clear_cloud_task_binding(cx);
            assert!(pickers.cloud_task_choice().is_none());
        });
        drop(receive);
    }

    #[gpui::test]
    fn model_uses_its_connected_agent_without_a_global_account_choice(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new());
        let pickers = cx.new(|cx| Pickers::new(state, cx));
        pickers.update(cx, |pickers, _| {
            pickers.cloud.owner = Some("owner".into());
            pickers.cloud.context = json!({"accountId":"owner","ownerId":"owner","organizationId":null,"agentConnections":[
                {"connectionId":"claude-connection","agent":"claude","state":"connected"},
                {"connectionId":"codex-connection","agent":"codex","state":"connected"},
                {"connectionId":"grok-connection","agent":"grok","state":"connected"}
            ]});
            for (agent, expected) in [
                (HarnessId::Codex, "codex-connection"),
                (HarnessId::ClaudeCode, "claude-connection"),
                (HarnessId::Grok, "grok-connection"),
                (HarnessId::Codex, "codex-connection"),
            ] {
                assert_eq!(
                    pickers.cloud_connection_for(agent).as_deref(),
                    Some(expected)
                );
            }
            pickers.cloud.context["agentConnections"][1]["state"] = json!("revoked");
            assert!(pickers.cloud_connection_for(HarnessId::Codex).is_none());
            assert_eq!(
                pickers
                    .cloud_connection_for(HarnessId::ClaudeCode)
                    .as_deref(),
                Some("claude-connection")
            );
        });
    }

    #[gpui::test]
    fn clicking_unprepared_cloud_project_selects_it_without_opening_settings(
        cx: &mut gpui::TestAppContext,
    ) {
        let opened_settings = Rc::new(Cell::new(false));
        let observed_settings = opened_settings.clone();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            cx.on_action(move |_: &crate::shell::OpenCloudSettings, _| {
                observed_settings.set(true);
            });
        });
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
                state
            });
            ProjectRows(cx.new(|cx| {
                let mut pickers = Pickers::new(state, cx);
                pickers.cloud.owner = Some("fixture-user".into());
                pickers.cloud.loaded = true;
                pickers.cloud.rows = vec![json!({
                    "projectId":"pending-project",
                    "accountId":"fixture-user",
                    "providerId":"boxd",
                    "repositoryIdentity":"fixture/project",
                    "displayName":"Pending project",
                    "prepared":false,
                    "state":"pending"
                })];
                pickers.open.open(PickerKind::Space);
                pickers
            }))
        });
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(handle.into(), cx);
            let row = visual
                .debug_bounds("pending-cloud-project-row-fixture")
                .expect("the real cloud-project row is painted");
            visual.simulate_click(row.center(), gpui::Modifiers::default());
        }
        cx.run_until_parked();
        handle
            .read_with(cx, |host, cx| {
                let pickers = host.0.read(cx);
                assert_eq!(
                    pickers.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                    "pending-project",
                    "mouse selection keeps an unprepared project available to the composer"
                );
                assert_eq!(
                    pickers.open_kind(),
                    None,
                    "successful selection closes the menu"
                );
            })
            .unwrap();
        assert!(
            !opened_settings.get(),
            "an unprepared environment is not a reason to send the user to Settings"
        );
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
    }
}
