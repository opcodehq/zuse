//! New Cloud chats use the runtime's published choices and the ordinary model menu.
use super::*;
use zuse_protocol::{
    cloud_catalog::CloudAgentCatalog,
    presentation::{PickerCatalog, ProviderCatalog},
};

#[derive(Default)]
pub(crate) struct State {
    pub(crate) providers: Vec<Value>,
    pub(crate) catalog: PickerCatalog,
    defaults: HashMap<String, String>,
    aliases: HashMap<(String, String), String>,
    pub(crate) error: Option<String>,
}

impl WorkspaceView {
    pub(crate) fn cloud_startup_pending(&self) -> bool {
        self.cloud.queued_prompt.is_some() && self.cloud.status["state"] != "ready"
    }

    pub(crate) fn has_cloud_creation_notice(&self) -> bool {
        self.cloud_startup_pending()
            || self.cloud_models_active()
                && (self.cloud.notice.is_some()
                    || (!(self.cloud_controller && self.cloud.account.is_none())
                        && !self.cloud_model_ready()))
    }

    pub(crate) fn cloud_creation_notice(&self, cx: &mut Context<Self>) -> AnyElement {
        let issue = super::preparation::issue(
            self.cloud.status["statusCode"]
                .as_str()
                .or_else(|| {
                    self.cloud
                        .target
                        .as_ref()
                        .and_then(|target| target["statusCode"].as_str())
                })
                .unwrap_or_default(),
        );
        let admitted = self
            .cloud
            .target
            .as_ref()
            .and_then(|target| target["workspaceId"].as_str())
            .or_else(|| self.cloud.status["workspaceId"].as_str())
            .is_some_and(|id| !id.is_empty());
        let launch_retry =
            issue.is_none() && self.cloud.launch.is_some() && self.cloud.create.is_none();
        let connection_retry = issue.is_none()
            && self.cloud.target.is_some()
            && (self.cloud.status_request.is_none()
                || matches!(
                    self.cloud.status["state"].as_str(),
                    Some("failed" | "blocked")
                ));
        let model_retry = !self.cloud_model_ready() && !self.cloud_models_loading();
        let setup_required = model_retry
            && self.cloud_models.providers.is_empty()
            && self.cloud.capabilities.is_object();
        if let Some(prompt) = self.cloud.queued_prompt.clone() {
            let busy = issue.is_none()
                && (self.cloud.create.is_some()
                    || self.cloud.status_request.is_some() && !connection_retry);
            let status = issue
                .map(|issue| issue.detail_for_admission(admitted).to_owned())
                .or_else(|| {
                    self.cloud.status["message"]
                        .as_str()
                        .filter(|message| !message.is_empty())
                        .map(str::to_owned)
                })
                .or_else(|| self.cloud.notice.clone())
                .unwrap_or_else(|| "Preparing cloud workspace…".into());
            return column()
                .w_full()
                .min_w_0()
                .child(crate::cloud_queue::queue(
                    prompt,
                    status,
                    busy,
                    true,
                    rgb(crate::shell::TEXT).into(),
                    rgb(MUTED).into(),
                    rgb(BORDER).opacity(0.4).into(),
                ))
                .when_some(
                    issue.and_then(|issue| issue.recovery.label()),
                    |view, action| {
                        view.child(row().w_full().px_3().py_1().justify_end().child(
                            control("cloud-preparation-recovery", action).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.settings = true;
                                    this.select_settings_section(
                                        crate::settings_view::Section::Cloud,
                                        cx,
                                    );
                                    cx.notify();
                                },
                            )),
                        ))
                    },
                )
                .when(launch_retry || connection_retry, |view| {
                    view.child(
                        row().w_full().px_3().py_1().justify_end().child(
                            control("retry-cloud-creation", "Retry")
                                .h(px(24.))
                                .tab_index(0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if connection_retry {
                                        this.connect_cloud();
                                        cx.notify();
                                    } else {
                                        this.submit_cloud(cx);
                                    }
                                })),
                        ),
                    )
                })
                .into_any_element();
        }
        let title = issue
            .map(|issue| issue.detail_for_admission(admitted).to_owned())
            .or_else(|| self.cloud.notice.clone())
            .or_else(|| self.cloud_models.error.clone())
            .unwrap_or_else(|| {
                if self.cloud_models_loading() {
                    "Loading Cloud models…"
                } else {
                    "Choose a Cloud model before starting."
                }
                .into()
            });
        row()
            .id("cloud-creation-notice")
            .role(Role::Status)
            .w_full()
            .min_h(px(32.))
            .px_3()
            .py_1()
            .gap_2()
            .child(icon("cloud-dither").size(px(14.)))
            .child(label(title).flex_1().min_w_0().text_size(px(12.)))
            .when_some(
                issue.and_then(|issue| issue.recovery.label()),
                |view, action| {
                    view.child(
                        control("cloud-preparation-recovery", action).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.settings = true;
                                this.select_settings_section(
                                    crate::settings_view::Section::Cloud,
                                    cx,
                                );
                                cx.notify();
                            },
                        )),
                    )
                },
            )
            .when(
                issue.is_none() && (launch_retry || connection_retry || model_retry),
                |view| {
                    view.child(
                        control(
                            "retry-cloud-creation",
                            if setup_required {
                                "Review setup"
                            } else {
                                "Retry"
                            },
                        )
                        .h(px(24.))
                        .tab_index(0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if connection_retry {
                                this.connect_cloud();
                                cx.notify();
                            } else if launch_retry {
                                this.submit_cloud(cx);
                            } else if setup_required {
                                this.settings = true;
                                this.select_settings_section(
                                    crate::settings_view::Section::Cloud,
                                    cx,
                                );
                                cx.notify();
                            } else {
                                this.refresh_cloud();
                                cx.notify();
                            }
                        })),
                    )
                },
            )
            .into_any_element()
    }

    pub(crate) fn cloud_models_active(&self) -> bool {
        self.cloud.mode && self.selected.is_none()
    }

    pub(crate) fn cloud_model_configuration_available(&self) -> bool {
        !self.cloud_models_active()
            || self
                .cloud
                .projects
                .get(self.cloud.project)
                .is_some_and(|project| project["supportsModelConfiguration"] == true)
    }

    pub(crate) fn cloud_model_change_pending(&self) -> bool {
        self.cloud_models_active() && (self.cloud.launch.is_some() || self.cloud.target.is_some())
    }

    pub(crate) fn cloud_models_loading(&self) -> bool {
        self.cloud_models_active()
            && self.cloud.context.is_some()
            && self.cloud_models.providers.is_empty()
    }

    pub(crate) fn receive_cloud_models(&mut self, value: Value, cx: &mut Context<Self>) {
        match serde_json::from_value::<CloudAgentCatalog>(value) {
            Ok(catalog) => {
                let previous = self
                    .composer_provider()
                    .and_then(|provider| provider["id"].as_str())
                    .map(str::to_owned);
                let mut next = State::default();
                for provider in catalog.providers {
                    let id = serde_json::to_value(provider.id)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned();
                    next.providers.push(json!({"id":id,"name":provider.name,"installed":true,"enabled":true,"supportsSteering":false,"steeringMode":"turn-boundary","reasoningLevels":[]}));
                    for alias in ["sonnet", "opus", "haiku", "fable"] {
                        if let Some(model) = provider.resolve_model(alias) {
                            next.aliases
                                .insert((id.clone(), alias.into()), model.id.clone());
                        }
                    }
                    next.defaults.insert(id.clone(), provider.default_model);
                    next.catalog.providers.insert(
                        id,
                        ProviderCatalog {
                            models: provider.models,
                            loaded: true,
                            ..Default::default()
                        },
                    );
                }
                if next.providers.is_empty() && self.cloud.capabilities.is_object() {
                    let providers = self.cloud.capabilities["providers"].as_array();
                    next.error=Some(if !providers.is_some_and(|rows|rows.iter().any(|p|p["configured"]==true)) {
                        "Cloud providers need server configuration before a computer can start."
                    } else if self.cloud.compute_provider.is_none() {
                        "A managed computer environment must be published before this task can start."
                    } else if !self.cloud.connections.iter().any(|connection|connection["state"]=="connected") {
                        "Connect a coding agent in Cloud setup before starting."
                    } else {
                        "This computer has no compatible connected agent. Choose another computer or review Cloud setup."
                    }.into());
                }
                self.cloud_models = next;
                if self.cloud_models_active() {
                    self.reconcile_creation_model(previous, cx);
                }
            }
            Err(_) => {
                self.cloud_models.error =
                    Some("Couldn't load Cloud models. Retry when connected.".into())
            }
        }
        self.invalidate_picker_menu();
        self.refresh_picker_placeholder(cx);
    }

    pub(crate) fn set_creation_destination(&mut self, cloud: bool, cx: &mut Context<Self>) {
        let provider = self
            .composer_provider()
            .and_then(|provider| provider["id"].as_str())
            .map(str::to_owned);
        self.model = self
            .selected_model()
            .map(|model| model.id.clone())
            .or_else(|| self.model.clone());
        self.cloud.mode = cloud || self.cloud_controller;
        self.reconcile_creation_model(provider, cx);
    }

    fn reconcile_creation_model(&mut self, provider: Option<String>, cx: &mut Context<Self>) {
        let rows = self.composer_providers();
        let index = rows
            .iter()
            .position(|row| provider.as_deref() == row["id"].as_str())
            .or_else(|| {
                rows.iter()
                    .position(|row| row["installed"] != false && row["enabled"] != false)
            });
        let Some(index) = index else {
            return;
        };
        let id = rows[index]["id"].as_str().unwrap_or_default().to_owned();
        let same = provider.as_deref() == Some(id.as_str());
        self.provider_index = index;
        if same && self.cloud_models_active() {
            self.model = self
                .model
                .as_ref()
                .and_then(|model| self.resolve_cloud_model(&id, model));
        }
        if !same
            || (self.cloud_models_active()
                && (self.model.is_none() || self.selected_model().is_none()))
        {
            self.model = if self.cloud_models_active() {
                self.preferences
                    .default_models
                    .get(&id)
                    .and_then(|model| self.resolve_cloud_model(&id, model))
                    .or_else(|| self.cloud_models.defaults.get(&id).cloned())
            } else {
                self.preferences.default_models.get(&id).cloned()
            };
            self.restore_model_choices();
        }
        self.normalize_cloud_choices();
        self.invalidate_picker_menu();
        self.configure_reasoning_slider(cx);
    }

    pub(crate) fn cloud_model_ready(&self) -> bool {
        self.cloud_models_active() && self.selected_model().is_some()
    }

    fn resolve_cloud_model(&self, provider: &str, model: &str) -> Option<String> {
        self.cloud_models
            .catalog
            .providers
            .get(provider)?
            .models
            .iter()
            .find(|row| row.id == model)
            .map(|row| row.id.clone())
            .or_else(|| {
                self.cloud_models
                    .aliases
                    .get(&(provider.into(), model.into()))
                    .cloned()
            })
    }

    fn normalize_cloud_choices(&mut self) {
        if !self.cloud_models_active() {
            return;
        }
        if let (Some(mut config), Some(model)) =
            (self.current_model_config(), self.selected_model())
        {
            config.normalize_for(model);
            if model.reasoning_levels.is_empty() {
                config.reasoning = None;
            }
            self.reasoning = config.reasoning;
            self.model_options = config.model_options;
        }
    }

    pub(crate) fn apply_cloud_default_model(&mut self) {
        if self.model.is_some() {
            return;
        }
        let preferred = self.preferences.starting_model();
        let index = preferred
            .as_ref()
            .and_then(|(provider, _)| {
                self.cloud_models
                    .providers
                    .iter()
                    .position(|row| row["id"] == *provider)
            })
            .unwrap_or(0);
        let Some(provider) = self
            .cloud_models
            .providers
            .get(index)
            .and_then(|row| row["id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        self.provider_index = index;
        self.model = preferred
            .filter(|(id, _)| id == &provider)
            .and_then(|(_, model)| model)
            .and_then(|model| self.resolve_cloud_model(&provider, &model))
            .or_else(|| self.cloud_models.defaults.get(&provider).cloned());
        self.restore_model_choices();
        self.normalize_cloud_choices();
    }
}
