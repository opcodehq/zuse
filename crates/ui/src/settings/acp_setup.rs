//! Adding and managing ACP agents inside the provider browser: pick one from
//! the public registry (or bring your own command), get it installed, fill in
//! how to start it, then add it and check that it answers — all in the detail
//! pane, with the added agent selected at the end.
use super::*;
use std::sync::Arc;
use zuse_protocol::custom_acp::{
    CustomAcpAgent, CustomAcpAgents,
    catalog::{self, Catalog, Offer, RegistryAgent},
    command,
};

/// How a chosen agent gets onto this computer.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Source {
    /// Runs through `npx`/`uvx`, downloaded on first start.
    Package {
        runner: &'static str,
        package: String,
    },
    /// A program to install by hand before it can start.
    Manual {
        command: String,
        repository: Option<String>,
    },
    /// The user's own command.
    Custom,
    /// Editing an agent that is already added.
    Existing,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Status {
    Idle,
    Saving,
    Testing,
    Failed(String),
}

pub(super) struct AcpState {
    /// The detail pane shows the add flow instead of a provider.
    pub open: bool,
    pub catalog: Loadable<Catalog>,
    pub icons: std::collections::HashMap<String, Arc<gpui::Image>>,
    pub query: Entity<ComposerInput>,
    pub choice: Option<(Option<RegistryAgent>, Source)>,
    /// The id the agent is saved under (new from the registry entry, or the
    /// existing agent's when editing).
    pub id: Option<HarnessId>,
    pub name: Entity<ComposerInput>,
    pub command: Entity<ComposerInput>,
    pub args: Entity<ComposerInput>,
    pub env: Entity<ComposerInput>,
    pub snapshot: Option<CustomAcpAgents>,
    pub status: Status,
    /// Whether each added agent answered its last check.
    pub checks: std::collections::HashMap<HarnessId, Result<usize, String>>,
    pub task: Option<Task<()>>,
    pub catalog_task: Option<Task<()>>,
    pub agents_task: Option<Task<()>>,
    pub icon_task: Option<Task<()>>,
    pub _subscriptions: Vec<gpui::Subscription>,
}

impl AcpState {
    pub fn new(cx: &mut Context<HarnessesPage>) -> Self {
        let query = cx.new(|cx| ComposerInput::new("Search agents", cx));
        let name = cx.new(|cx| ComposerInput::new("My agent", cx));
        let command = cx.new(|cx| ComposerInput::new("my-agent", cx));
        let args = cx.new(|cx| ComposerInput::new("--acp", cx));
        let env = cx.new(|cx| ComposerInput::new("API_KEY=… OTHER=…", cx));
        let subscriptions = vec![cx.observe(&query, |_, _, cx| cx.notify())];
        Self {
            open: false,
            catalog: Loadable::Idle,
            icons: Default::default(),
            query,
            choice: None,
            id: None,
            name,
            command,
            args,
            env,
            snapshot: None,
            status: Status::Idle,
            checks: Default::default(),
            task: None,
            catalog_task: None,
            agents_task: None,
            icon_task: None,
            _subscriptions: subscriptions,
        }
    }
}

fn source_for(agent: &RegistryAgent, offer: &Offer) -> Source {
    match offer {
        Offer::Add(custom) if custom.command == "npx" || custom.command == "uvx" => {
            Source::Package {
                runner: if custom.command == "npx" {
                    "npx"
                } else {
                    "uvx"
                },
                package: agent
                    .distribution
                    .npx
                    .as_ref()
                    .or(agent.distribution.uvx.as_ref())
                    .map(|p| p.package.clone())
                    .unwrap_or_default(),
            }
        }
        Offer::Add(_) => Source::Custom,
        Offer::Manual(custom) => Source::Manual {
            command: custom.command.clone(),
            repository: agent.repository.clone(),
        },
        Offer::BuiltIn(_) => Source::Custom,
    }
}

/// The engine's probe errors in words: what went wrong and what to do.
fn friendly_error(command: &str, raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("missing_executable")
        || lower.contains("binary not found")
        || lower.contains("no such file")
    {
        format!("`{command}` isn't installed, or isn't on the PATH Zuse uses.")
    } else if lower.contains("permission denied") {
        format!("`{command}` can't be run — check its permissions.")
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "The agent started but didn't answer in time. It may still be downloading or waiting for a login.".into()
    } else if lower.contains("auth") || lower.contains("login") || lower.contains("api key") {
        "The agent started but needs you to sign in or set an API key (add it under Environment)."
            .into()
    } else {
        raw.trim().trim_start_matches("model discovery ").to_owned()
    }
}

impl HarnessesPage {
    pub(super) fn open_acp(&mut self, cx: &mut Context<Self>) {
        self.acp.open = true;
        self.acp.choice = None;
        self.acp.status = Status::Idle;
        self.acp.query.update(cx, |i, cx| i.set_text("", cx));
        if matches!(self.acp.catalog, Loadable::Idle | Loadable::Error(_)) {
            self.load_acp_catalog(cx);
        }
        self.load_acp_agents(cx);
        cx.notify();
    }

    pub(super) fn close_acp(&mut self, cx: &mut Context<Self>) {
        self.acp.open = false;
        self.acp.choice = None;
        cx.notify();
    }

    fn acp_call(
        &self,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<serde_json::Value, String>, &mut Context<Self>) + 'static,
    ) -> Option<Task<()>> {
        let engine = self.state.read(cx).engine().cloned()?;
        let params = self.with_target(params);
        Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string());
            this.update(cx, |page, cx| {
                done(page, result, cx);
                cx.notify();
            })
            .ok();
        }))
    }

    fn load_acp_catalog(&mut self, cx: &mut Context<Self>) {
        self.acp.catalog = Loadable::Loading;
        self.acp.catalog_task = self.acp_call(
            catalog::GET,
            serde_json::json!({}),
            cx,
            |page, result, cx| match result
                .and_then(|v| serde_json::from_value::<Catalog>(v).map_err(|e| e.to_string()))
            {
                Ok(catalog) => {
                    let ids: Vec<String> = catalog.agents.iter().map(|a| a.id.clone()).collect();
                    page.acp.catalog = Loadable::Ready(catalog);
                    page.load_acp_icons(ids, cx);
                }
                Err(error) => page.acp.catalog = Loadable::Error(error),
            },
        );
    }

    fn load_acp_icons(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target_device.clone();
        self.acp.icon_task = Some(cx.spawn(async move |this, cx| {
            for id in ids {
                let mut params = serde_json::json!({ "id": id });
                if let Some(target) = &target {
                    params["targetDeviceId"] = serde_json::json!(target);
                }
                let Ok(value) = engine.client().call(catalog::ICON, params).await else {
                    continue;
                };
                let Some(svg) = value.as_str().map(str::to_owned) else {
                    continue;
                };
                let image = Arc::new(gpui::Image::from_bytes(
                    gpui::ImageFormat::Svg,
                    catalog::tint_icon(&svg, "#d4d4d8").into_bytes(),
                ));
                if this
                    .update(cx, |page, cx| {
                        page.acp.icons.insert(id, image);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    pub(super) fn load_acp_agents(&mut self, cx: &mut Context<Self>) {
        self.acp.agents_task = self.acp_call(
            methods::GET_CUSTOM_ACP_AGENTS,
            serde_json::json!({}),
            cx,
            |page, result, _| {
                if let Ok(snapshot) =
                    result.and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
                {
                    page.acp.snapshot = Some(snapshot);
                }
            },
        );
    }

    fn choose_acp(&mut self, agent: Option<RegistryAgent>, cx: &mut Context<Self>) {
        let platform = match &self.acp.catalog {
            Loadable::Ready(c) => c.platform.clone(),
            _ => catalog::platform(),
        };
        let (prefill, source) = match &agent {
            Some(agent) => match catalog::offer(agent, &platform) {
                Some(Offer::BuiltIn(harness)) => {
                    // Already a provider: show it instead.
                    self.close_acp(cx);
                    self.select_provider(harness, cx);
                    return;
                }
                Some(offer) => {
                    let custom = match &offer {
                        Offer::Add(c) | Offer::Manual(c) => Some(c.clone()),
                        Offer::BuiltIn(_) => None,
                    };
                    (custom, source_for(agent, &offer))
                }
                None => (None, Source::Custom),
            },
            None => (None, Source::Custom),
        };
        let id = prefill
            .as_ref()
            .map(|c| c.id)
            .or_else(|| HarnessId::try_from(format!("acp-{}", uuid::Uuid::new_v4().simple())).ok());
        self.fill_acp_form(
            prefill.as_ref(),
            agent.as_ref().map(|a| a.name.as_str()),
            cx,
        );
        self.acp.id = id;
        self.acp.choice = Some((agent, source));
        self.acp.status = Status::Idle;
        cx.notify();
    }

    fn fill_acp_form(
        &mut self,
        agent: Option<&CustomAcpAgent>,
        name: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let name = agent
            .map(|a| a.name.clone())
            .or(name.map(str::to_owned))
            .unwrap_or_default();
        let command_line = agent.map(|a| a.command.clone()).unwrap_or_default();
        let args = agent
            .map(|a| command::arguments(&a.args))
            .unwrap_or_default();
        let env = agent
            .map(|a| {
                a.env
                    .iter()
                    .map(|(k, v)| format!("{k}={}", command::arguments(std::slice::from_ref(v))))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        self.acp.name.update(cx, |i, cx| i.set_text(name, cx));
        self.acp
            .command
            .update(cx, |i, cx| i.set_text(command_line, cx));
        self.acp.args.update(cx, |i, cx| i.set_text(args, cx));
        self.acp.env.update(cx, |i, cx| i.set_text(env, cx));
    }

    /// Edit an added agent in the same form.
    pub(super) fn edit_acp(&mut self, id: HarnessId, cx: &mut Context<Self>) {
        let Some(agent) = self
            .acp
            .snapshot
            .as_ref()
            .and_then(|s| s.agents.iter().find(|a| a.id == id))
            .cloned()
        else {
            return;
        };
        self.fill_acp_form(Some(&agent), None, cx);
        self.acp.id = Some(id);
        self.acp.choice = Some((None, Source::Existing));
        self.acp.status = Status::Idle;
        self.acp.open = true;
        cx.notify();
    }

    fn form_agent(&self, cx: &Context<Self>) -> Result<CustomAcpAgent, String> {
        let id = self.acp.id.ok_or("Pick an agent first.")?;
        let args = command::words(self.acp.args.read(cx).text())?;
        let mut env = std::collections::BTreeMap::new();
        for pair in command::words(self.acp.env.read(cx).text())? {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("“{pair}” needs NAME=value."))?;
            env.insert(key.to_owned(), value.to_owned());
        }
        let model_ids = self
            .acp
            .snapshot
            .as_ref()
            .and_then(|s| s.agents.iter().find(|a| a.id == id))
            .map(|a| a.model_ids.clone())
            .unwrap_or_default();
        let agent = CustomAcpAgent {
            id,
            name: self.acp.name.read(cx).text().trim().to_owned(),
            command: self.acp.command.read(cx).text().trim().to_owned(),
            args,
            env,
            model_ids,
        };
        agent.validate()?;
        Ok(agent)
    }

    fn save_acp(&mut self, cx: &mut Context<Self>) {
        if matches!(self.acp.status, Status::Saving | Status::Testing) {
            return;
        }
        let agent = match self.form_agent(cx) {
            Ok(agent) => agent,
            Err(error) => {
                self.acp.status = Status::Failed(error);
                cx.notify();
                return;
            }
        };
        let Some(revision) = self.acp.snapshot.as_ref().map(|s| s.revision) else {
            self.acp.status =
                Status::Failed("Still loading your agents — try again in a moment.".into());
            cx.notify();
            return;
        };
        let id = agent.id;
        self.acp.status = Status::Saving;
        self.acp.task = self.acp_call(
            methods::SET_CUSTOM_ACP_AGENT,
            serde_json::json!({ "agent": agent, "expectedRevision": revision }),
            cx,
            move |page, result, cx| match result.and_then(|v| {
                serde_json::from_value::<CustomAcpAgents>(v).map_err(|e| e.to_string())
            }) {
                Ok(snapshot) => {
                    page.acp.snapshot = Some(snapshot);
                    // It's saved either way: list it now, then check it.
                    page.load(cx);
                    page.test_acp(id, true, cx);
                }
                Err(error) => page.acp.status = Status::Failed(error),
            },
        );
        cx.notify();
    }

    /// Start the agent and ask for its models. `then_select` closes the add
    /// flow on success and shows the agent.
    pub(super) fn test_acp(&mut self, id: HarnessId, then_select: bool, cx: &mut Context<Self>) {
        if then_select {
            self.acp.status = Status::Testing;
        }
        self.acp.checks.remove(&id);
        self.acp.task = self.acp_call(
            methods::LIST_MODELS,
            serde_json::json!({ "harness": id, "force": true }),
            cx,
            move |page, result, cx| {
                let command = page
                    .acp
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.agents.iter().find(|a| a.id == id))
                    .map(|a| a.command.clone())
                    .unwrap_or_default();
                let check = result
                    .and_then(|v| {
                        serde_json::from_value::<Vec<zuse_protocol::Model>>(v)
                            .map(|m| m.len())
                            .map_err(|_| {
                                "The agent answered with something that isn't a model list.".into()
                            })
                    })
                    .map_err(|raw| friendly_error(&command, &raw));
                page.acp.checks.insert(id, check.clone());
                if then_select {
                    match check {
                        Ok(_) => {
                            page.acp.status = Status::Idle;
                            page.acp.open = false;
                            page.acp.choice = None;
                            page.pending_select = Some(id);
                            page.load(cx);
                        }
                        Err(error) => page.acp.status = Status::Failed(error),
                    }
                }
            },
        );
        cx.notify();
    }

    pub(super) fn remove_acp(&mut self, id: HarnessId, cx: &mut Context<Self>) {
        let Some(revision) = self.acp.snapshot.as_ref().map(|s| s.revision) else {
            return;
        };
        self.acp.task = self.acp_call(
            methods::DELETE_CUSTOM_ACP_AGENT,
            serde_json::json!({ "harness": id, "expectedRevision": revision }),
            cx,
            move |page, result, cx| {
                if let Ok(snapshot) =
                    result.and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
                {
                    page.acp.snapshot = Some(snapshot);
                    page.expanded_harness = None;
                    page.load(cx);
                }
            },
        );
    }

    // ── rendering ───────────────────────────────────────────────────────────

    pub(super) fn render_acp_flow(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let back = |label: &'static str, id: &'static str| {
            widgets::text_action(theme, widgets::ActionTone::Quiet, label).id(id)
        };
        let header = |title: &str, subtitle: &str| {
            div()
                .px(px(24.))
                .pt(px(20.))
                .pb(px(16.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_strong)
                        .child(SharedString::from(title.to_owned())),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(subtitle.to_owned())),
                )
        };
        match self.acp.choice.clone() {
            None => div()
                .flex()
                .flex_col()
                .child(header(
                    "Add an ACP agent",
                    "Any coding agent that speaks the Agent Client Protocol can run in Zuse. Pick one, or bring your own command.",
                ))
                .child(self.acp_chooser(theme, cx))
                .into_any_element(),
            Some((agent, source)) => {
                let title = match (&agent, &source) {
                    (_, Source::Existing) => format!("Edit {}", self.acp.name.read(cx).text()),
                    (Some(agent), _) => format!("Set up {}", agent.name),
                    (None, _) => "Set up your own agent".to_owned(),
                };
                let subtitle = agent
                    .as_ref()
                    .map(|a| a.description.clone())
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| "Tell Zuse how to start the agent. It talks ACP over stdin and stdout.".into());
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(16.))
                            .pt(px(12.))
                            .child(
                                back(
                                    if source == Source::Existing { "← Back" } else { "← All agents" },
                                    "acp-back",
                                )
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    if matches!(page.acp.choice, Some((_, Source::Existing))) {
                                        page.close_acp(cx);
                                    } else {
                                        page.acp.choice = None;
                                        cx.notify();
                                    }
                                })),
                            ),
                    )
                    .child(header(&title, &subtitle))
                    .child(self.acp_steps(agent.as_ref(), &source, theme, cx))
                    .into_any_element()
            }
        }
    }

    fn acp_icon(&self, id: &str, theme: &Theme) -> AnyElement {
        let tile = div()
            .flex_none()
            .size(px(32.))
            .rounded(px(8.))
            .bg(theme.settings_frame_bg().opacity(0.9))
            .flex()
            .items_center()
            .justify_center();
        match self.acp.icons.get(id) {
            Some(image) => tile
                .child(gpui::img(image.clone()).size(px(18.)))
                .into_any_element(),
            None => tile
                .child(
                    crate::icons::icon(crate::icons::SETTINGS_PROVIDERS)
                        .size(px(16.))
                        .text_color(theme.text_muted),
                )
                .into_any_element(),
        }
    }

    fn acp_chooser(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let query = self.acp.query.read(cx).text().to_owned();
        let platform = match &self.acp.catalog {
            Loadable::Ready(c) => c.platform.clone(),
            _ => catalog::platform(),
        };
        let added: std::collections::HashSet<HarnessId> = self
            .acp
            .snapshot
            .as_ref()
            .map(|s| s.agents.iter().map(|a| a.id).collect())
            .unwrap_or_default();
        let custom_row = div()
            .id("acp-custom")
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(10.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.settings_frame_bg().opacity(0.5)))
            .on_click(cx.listener(|page, _, _, cx| page.choose_acp(None, cx)))
            .child(
                div()
                    .flex_none()
                    .size(px(32.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::icons::icon(crate::icons::PLUS)
                            .size(px(14.))
                            .text_color(theme.text),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(widgets::row_title(theme, "Your own command"))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.text_muted)
                            .child("Any program you can run that speaks ACP."),
                    ),
            );
        let list: AnyElement = match &self.acp.catalog {
            Loadable::Idle | Loadable::Loading => div()
                .p(px(14.))
                .child(popover::skeleton_rows(
                    "acp-catalog-skeleton",
                    theme,
                    5,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Loadable::Error(error) => div()
                .p(px(14.))
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child(SharedString::from(format!(
                    "Couldn't load the agent list ({error}). You can still add your own command."
                )))
                .into_any_element(),
            Loadable::Ready(catalog) => {
                let rows = catalog
                    .agents
                    .iter()
                    .filter(|a| query.trim().is_empty() || catalog::matches(a, &query))
                    .enumerate()
                    .map(|(ix, agent)| {
                        let offer = catalog::offer(agent, &platform);
                        let already = match &offer {
                            Some(Offer::Add(c) | Offer::Manual(c)) => added.contains(&c.id),
                            _ => false,
                        };
                        let (chip, tone): (&str, Option<gpui::Hsla>) = match &offer {
                            _ if already => ("Added", Some(theme.success)),
                            Some(Offer::BuiltIn(_)) => ("Built in", None),
                            Some(Offer::Add(c)) if c.command == "npx" => ("Runs with npx", None),
                            Some(Offer::Add(c)) if c.command == "uvx" => ("Runs with uvx", None),
                            Some(Offer::Add(_)) => ("Ready", None),
                            Some(Offer::Manual(_)) => ("Install first", Some(theme.warning)),
                            None => ("Not for this OS", None),
                        };
                        let usable = offer.is_some();
                        let agent_for_click = agent.clone();
                        div()
                            .id(("acp-agent", ix))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(14.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(widgets::row_divider(theme))
                            .when(usable, |el| {
                                el.cursor_pointer()
                                    .hover(|s| s.bg(theme.settings_frame_bg().opacity(0.5)))
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.choose_acp(Some(agent_for_click.clone()), cx)
                                    }))
                            })
                            .when(!usable, |el| el.opacity(0.5))
                            .child(self.acp_icon(&agent.id, theme))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(widgets::row_title(theme, agent.name.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.5))
                                            .text_color(theme.text_muted)
                                            .child(agent.description.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px(px(8.))
                                    .py(px(2.))
                                    .rounded_full()
                                    .text_size(px(10.5))
                                    .bg(tone.map_or(theme.wash(0.08), |c| c.opacity(0.12)))
                                    .text_color(tone.unwrap_or(theme.text_muted))
                                    .child(chip),
                            )
                    })
                    .collect::<Vec<_>>();
                let empty = rows.is_empty();
                div()
                    .children(rows)
                    .when(empty, |el| {
                        el.child(
                            div()
                                .p(px(14.))
                                .border_t_1()
                                .border_color(widgets::row_divider(theme))
                                .text_size(px(12.))
                                .text_color(theme.text_muted)
                                .child(
                                    "No agent matches. Try another name, or add your own command.",
                                ),
                        )
                    })
                    .into_any_element()
            }
        };
        div()
            .px(px(24.))
            .pb(px(24.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(popover::dialog_field(
                self.acp.query.clone().into_any_element(),
            ))
            .child(
                widgets::section_card(theme)
                    .mt_0()
                    .child(custom_row)
                    .child(list),
            )
            .into_any_element()
    }

    fn step(theme: &Theme, number: usize, title: &str, done: bool, body: AnyElement) -> gpui::Div {
        div()
            .flex()
            .gap(px(14.))
            .child(
                div()
                    .flex_none()
                    .size(px(24.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .when(done, |el| {
                        el.bg(theme.success.opacity(0.18)).text_color(theme.success)
                    })
                    .when(!done, |el| {
                        el.bg(theme.settings_frame_bg()).text_color(theme.text)
                    })
                    .child(SharedString::from(number.to_string())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .pt(px(3.))
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(SharedString::from(title.to_owned())),
                    )
                    .child(body),
            )
    }

    fn field(theme: &Theme, label: &str, hint: &str, input: AnyElement, mono: bool) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(widgets::field_label(theme, label.to_owned()))
            .child(
                popover::dialog_field(input)
                    .when(mono, |el| el.font_family(theme.font_mono.clone()))
                    .text_size(px(12.5)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(hint.to_owned())),
            )
    }

    fn acp_steps(
        &self,
        agent: Option<&RegistryAgent>,
        source: &Source,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let note = |text: String| {
            div()
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child(SharedString::from(text))
        };
        let code = |text: String| {
            div()
                .rounded(px(7.))
                .bg(theme.settings_frame_bg().opacity(0.7))
                .px(px(10.))
                .py(px(6.))
                .font_family(theme.font_mono.clone())
                .text_size(px(11.5))
                .child(SharedString::from(text))
        };
        let install: Option<AnyElement> = match source {
            Source::Package { runner, package } => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(note(match *runner {
                        "npx" => "Nothing to install by hand. Zuse starts it with npx, which comes with Node.js; the first start downloads the package.".into(),
                        _ => "Nothing to install by hand. Zuse starts it with uvx, which comes with uv; the first start downloads the package.".into(),
                    }))
                    .child(code(format!("{runner} {package}")))
                    .into_any_element(),
            ),
            Source::Manual { command, repository } => {
                let repository = repository.clone();
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(note(format!(
                            "Install it first, so that `{command}` runs from a terminal on this computer."
                        )))
                        .when_some(repository, |el, url| {
                            el.child(
                                div().flex().child(
                                    widgets::text_action(theme, widgets::ActionTone::Outlined, "Open install instructions")
                                        .id("acp-open-repo")
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                ),
                            )
                        })
                        .into_any_element(),
                )
            }
            Source::Custom => Some(
                note("Install the agent so its command runs from a terminal on this computer.".into()).into_any_element(),
            ),
            Source::Existing => None,
        };
        let status = self.acp.status.clone();
        let busy = matches!(status, Status::Saving | Status::Testing);
        let action = match (&status, source) {
            (Status::Saving, _) => "Saving…",
            (Status::Testing, _) => "Starting the agent…",
            (_, Source::Existing) => "Save and check",
            _ => "Add and check",
        };
        let result: Option<AnyElement> = match &status {
            Status::Failed(error) => Some(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.danger.opacity(0.25))
                    .bg(theme.danger.opacity(0.06))
                    .px(px(12.))
                    .py(px(8.))
                    .child(crate::icons::icon(crate::icons::DANGER_TRIANGLE).size(px(14.)).text_color(theme.danger_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .text_size(px(11.5))
                            .child(div().text_color(theme.danger_muted).child(SharedString::from(error.clone())))
                            .child(div().text_color(theme.text_muted).child("Fix it above and try again. The agent is saved, so you can also come back to it from the list.")),
                    )
                    .into_any_element(),
            ),
            _ => None,
        };
        let mut n = 0;
        let mut next = || {
            n += 1;
            n
        };
        let _ = agent;
        div()
            .px(px(24.))
            .pb(px(24.))
            .flex()
            .flex_col()
            .gap(px(22.))
            .when_some(install, |el, body| el.child(Self::step(theme, next(), "Get the agent", false, body)))
            .child(Self::step(
                theme,
                next(),
                "How Zuse starts it",
                false,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(Self::field(theme, "Name", "Shown in the agent list and the composer.", self.acp.name.clone().into_any_element(), false))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(Self::field(theme, "Command", "A program on your PATH, or a full path.", self.acp.command.clone().into_any_element(), true).flex_1())
                            .child(Self::field(theme, "Arguments", "Separated by spaces; quote ones with spaces.", self.acp.args.clone().into_any_element(), true).flex_1()),
                    )
                    .child(Self::field(theme, "Environment", "Optional NAME=value pairs, e.g. an API key the agent needs.", self.acp.env.clone().into_any_element(), true))
                    .into_any_element(),
            ))
            .child(Self::step(
                theme,
                next(),
                if *source == Source::Existing { "Save and check" } else { "Add and check" },
                false,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(note("Zuse starts the agent once and asks for its models, so you know it works before your first chat.".into()))
                    .children(result)
                    .child(
                        div().flex().gap(px(8.)).child(
                            widgets::text_action(theme, widgets::ActionTone::Filled, action)
                                .id("acp-save")
                                .when(busy, |el| el.opacity(0.6))
                                .on_click(cx.listener(|page, _, _, cx| page.save_acp(cx))),
                        ),
                    )
                    .into_any_element(),
            ))
            .into_any_element()
    }

    /// The Setup tab of an added ACP agent: how it starts, whether it
    /// answers, and edit/remove.
    pub(super) fn acp_agent_tab(
        &self,
        id: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent = self
            .acp
            .snapshot
            .as_ref()
            .and_then(|s| s.agents.iter().find(|a| a.id == id))
            .cloned();
        let Some(agent) = agent else {
            return div()
                .p(px(24.))
                .child(popover::skeleton_rows(
                    "acp-agent-skeleton",
                    theme,
                    2,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element();
        };
        let installed = match &self.harnesses {
            Loadable::Ready(list) => list.iter().any(|d| d.id == id && d.installed),
            _ => true,
        };
        let check = self.acp.checks.get(&id).cloned().or_else(|| {
            (!installed).then(|| Err(friendly_error(&agent.command, "missing_executable")))
        });
        let (color, line) = match &check {
            None => (theme.text_muted, "Not checked yet".to_owned()),
            Some(Ok(0)) => (
                theme.success,
                "Answers · no model list (uses its default model)".to_owned(),
            ),
            Some(Ok(n)) => (theme.success, format!("Answers · {n} models")),
            Some(Err(e)) => (theme.danger, e.clone()),
        };
        let command_line = std::iter::once(agent.command.clone())
            .chain(std::iter::once(command::arguments(&agent.args)).filter(|a| !a.is_empty()))
            .collect::<Vec<_>>()
            .join(" ");
        let env_keys = agent.env.keys().cloned().collect::<Vec<_>>().join(", ");
        div()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                widgets::section_card(theme)
                    .mt_0()
                    .child(
                        widgets::card_row(theme, true)
                            .child(div().flex_none().size(px(7.)).rounded_full().bg(color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(widgets::row_title(theme, line)),
                            )
                            .child(
                                widgets::text_action(
                                    theme,
                                    widgets::ActionTone::Outlined,
                                    "Check now",
                                )
                                .id("acp-check")
                                .on_click(
                                    cx.listener(move |page, _, _, cx| page.test_acp(id, false, cx)),
                                ),
                            ),
                    )
                    .child(
                        widgets::card_row(theme, false).child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.text_muted)
                                        .child("Starts with"),
                                )
                                .child(
                                    div()
                                        .font_family(theme.font_mono.clone())
                                        .text_size(px(12.))
                                        .text_color(theme.text)
                                        .child(SharedString::from(command_line)),
                                )
                                .when(!env_keys.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme.text_muted)
                                            .child(SharedString::from(format!(
                                                "Environment: {env_keys}"
                                            ))),
                                    )
                                }),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        widgets::text_action(theme, widgets::ActionTone::Outlined, "Edit")
                            .id("acp-edit")
                            .on_click(cx.listener(move |page, _, _, cx| page.edit_acp(id, cx))),
                    )
                    .child(
                        widgets::text_action(theme, widgets::ActionTone::Quiet, "Remove agent")
                            .id("acp-remove")
                            .text_color(theme.danger_muted)
                            .on_click(cx.listener(move |page, _, _, cx| page.remove_acp(id, cx))),
                    ),
            )
            .into_any_element()
    }
}
