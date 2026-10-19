//! Device-scoped ACP executable setup over the same RPC used by paired clients.
//!
//! "Add ACP agent" opens a dialog over the public ACP registry
//! ([`acp_registry`]); adding an entry saves a custom agent through the same
//! `SET_CUSTOM_ACP_AGENT` call as the hand-written form, which stays one
//! click away as "Use a custom command…".
use crate::{
    composer::ComposerInput, popover, settings::plugins::catalog_ui, settings::widgets,
    state::AppState, theme::Theme,
};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, Focusable, Subscription, Task, Window, div,
    http_client::HttpClient, prelude::*, px,
};
use serde_json::json;
use std::{collections::HashMap, sync::Arc};
use zuse_protocol::custom_acp::{CustomAcpAgent, CustomAcpAgents};
use zuse_rpc::methods;

#[path = "acp_registry.rs"]
pub(crate) mod acp_registry;
use acp_registry::{Offer, RegistryAgent};

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
    /// The Add ACP agent dialog.
    dialog_open: bool,
    search: Entity<ComposerInput>,
    _search: Subscription,
    /// The registry's agents once fetched (or the built-in fallback).
    registry: Option<Vec<RegistryAgent>>,
    registry_error: Option<String>,
    registry_task: Option<Task<()>>,
    /// Raw icon SVGs by registry id; `None` while loading or after a failure.
    icons: HashMap<String, Option<Arc<str>>>,
    /// Icons tinted for the current theme, keyed by id and colour.
    tinted: HashMap<String, (String, Arc<gpui::Image>)>,
    icon_task: Option<Task<()>>,
    /// The registry entry being added.
    adding: Option<String>,
    /// Review fixtures render seeded data without an engine.
    fixture: bool,
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

async fn fetch(client: Arc<dyn HttpClient>, url: String, limit: usize) -> Result<Vec<u8>, String> {
    use futures::AsyncReadExt;
    let mut response = client
        .get(&url, gpui::http_client::AsyncBody::empty(), true)
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("The registry answered {}", response.status()));
    }
    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .map_err(|error| error.to_string())?;
    if body.len() > limit {
        return Err("The registry response is too large".into());
    }
    Ok(body)
}

fn hex(color: gpui::Hsla) -> String {
    let rgba = gpui::Rgba::from(color);
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b)
    )
}

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
        let search = cx.new(|cx| {
            ComposerInput::new("Search agents…", cx)
                .with_single_line()
                .with_tab_stop()
        });
        let search_events = cx.observe(&search, |_: &mut Self, _, cx| cx.notify());
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
            dialog_open: false,
            search,
            _search: search_events,
            registry: None,
            registry_error: None,
            registry_task: None,
            icons: HashMap::new(),
            tinted: HashMap::new(),
            icon_task: None,
            adding: None,
            fixture: false,
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
        self.adding = None;
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
                card.adding = None;
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
            Ok(agent) => self.put(agent, cx),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }
    /// Save `agent` against the snapshot this card last read.
    fn put(&mut self, agent: CustomAcpAgent, cx: &mut Context<Self>) {
        match &self.snapshot {
            Some(snapshot) => {
                let revision = snapshot.revision;
                self.request(
                    methods::SET_CUSTOM_ACP_AGENT,
                    json!({"agent": agent, "expectedRevision": revision}),
                    cx,
                );
            }
            None => {
                self.error = Some("Custom agents are still loading. Try again in a moment.".into());
                cx.notify();
            }
        }
    }
    fn installed(&self, id: zuse_protocol::HarnessId) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.agents.iter().any(|agent| agent.id == id))
    }

    // ---- Add ACP agent dialog ----

    fn open_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog_open = true;
        self.search.update(cx, |input, cx| input.set_text("", cx));
        window.focus(&self.search.focus_handle(cx), cx);
        if self.registry.is_none() || self.registry_error.is_some() {
            self.fetch_registry(cx);
        }
        cx.notify();
    }
    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog_open = false;
        cx.notify();
    }
    fn fetch_registry(&mut self, cx: &mut Context<Self>) {
        if self.registry_task.is_some() {
            return;
        }
        self.registry_error = None;
        let client = cx.http_client();
        self.registry_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch(
                client,
                acp_registry::REGISTRY_URL.into(),
                acp_registry::MAX_REGISTRY_BYTES,
            )
            .await
            .and_then(|bytes| acp_registry::parse(&bytes));
            let _ = this.update(cx, |card, cx| {
                card.registry_task = None;
                match result {
                    Ok(agents) => {
                        card.registry = Some(agents);
                        card.fetch_icons(cx);
                    }
                    Err(error) => {
                        tracing::warn!("ACP registry unavailable: {error}");
                        card.registry_error = Some(error);
                        if card.registry.is_none() {
                            card.registry = Some(acp_registry::fallback());
                        }
                    }
                }
                cx.notify();
            });
        }));
    }
    /// One task walks the registry's icons; each lands as it arrives.
    fn fetch_icons(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<(String, String)> = self
            .registry
            .iter()
            .flatten()
            .filter(|agent| !self.icons.contains_key(&agent.id))
            .filter_map(|agent| {
                let url = agent.icon.as_deref()?;
                url.starts_with("https://")
                    .then(|| (agent.id.clone(), url.to_owned()))
            })
            .collect();
        if pending.is_empty() {
            return;
        }
        for (id, _) in &pending {
            self.icons.insert(id.clone(), None);
        }
        let client = cx.http_client();
        self.icon_task = Some(cx.spawn(async move |this, cx| {
            for (id, url) in pending {
                let svg = fetch(client.clone(), url, acp_registry::MAX_ICON_BYTES)
                    .await
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .filter(|svg| svg.contains("<svg"));
                if this
                    .update(cx, |card, cx| {
                        card.icons.insert(id, svg.map(Arc::from));
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }
    fn icon(&mut self, id: &str, color: &str) -> Option<Arc<gpui::Image>> {
        let svg = self.icons.get(id)?.clone()?;
        if let Some((tint, image)) = self.tinted.get(id)
            && tint == color
        {
            return Some(image.clone());
        }
        let image = Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Svg,
            acp_registry::tint_icon(&svg, color).into_bytes(),
        ));
        self.tinted
            .insert(id.to_owned(), (color.to_owned(), image.clone()));
        Some(image)
    }
    fn add_from_registry(
        &mut self,
        registry_id: String,
        agent: CustomAcpAgent,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.adding = Some(registry_id);
        self.put(agent, cx);
        if !self.busy {
            self.adding = None;
        }
    }

    fn render_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.dialog_open {
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let query = self.search.read(cx).text().to_owned();
        let platform = acp_registry::platform();
        let tint = hex(theme.text);
        // Agents to add come first; the built-in providers close the list.
        let mut agents: Vec<RegistryAgent> = self
            .registry
            .iter()
            .flatten()
            .filter(|agent| acp_registry::matches(agent, &query))
            .cloned()
            .collect();
        agents.sort_by_key(|agent| acp_registry::built_in(&agent.id).is_some());
        let focused = self.search.focus_handle(cx).is_focused(window);
        let muted_row = |label: &'static str| {
            div()
                .flex_none()
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(crate::typography::ui_rems(11.))
                .text_color(theme.text_muted)
                .child(label)
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, agent) in agents.into_iter().enumerate() {
            let icon = self.icon(&agent.id, &tint);
            let initial = agent
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            let tile = div()
                .size(px(28.))
                .flex_none()
                .rounded(px(7.))
                .bg(theme.settings_frame_bg().opacity(0.6))
                .flex()
                .items_center()
                .justify_center()
                .text_size(crate::typography::ui_rems(12.))
                .text_color(theme.text_muted)
                .child(match icon {
                    Some(image) => gpui::img(image).size(px(16.)).into_any_element(),
                    None => div().child(initial).into_any_element(),
                });
            let action: AnyElement = match acp_registry::offer(&agent, &platform) {
                Some(Offer::BuiltIn(_)) => muted_row("Built in").into_any_element(),
                Some(Offer::Add(custom) | Offer::Manual(custom)) if self.installed(custom.id) => {
                    muted_row("Added")
                        .child(
                            crate::icons::icon(crate::icons::CHECK)
                                .size(px(12.))
                                .text_color(theme.text_muted),
                        )
                        .into_any_element()
                }
                _ if self.adding.as_deref() == Some(agent.id.as_str()) => {
                    muted_row("Adding…").into_any_element()
                }
                Some(Offer::Add(custom)) => {
                    let registry_id = agent.id.clone();
                    widgets::text_action(&theme, widgets::ActionTone::Outlined, "Add")
                        .id(("acp-registry-add", index))
                        .flex_none()
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label(format!("Add {}", agent.name))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.add_from_registry(registry_id.clone(), custom.clone(), cx)
                        }))
                        .into_any_element()
                }
                Some(Offer::Manual(custom)) => {
                    widgets::text_action(&theme, widgets::ActionTone::Outlined, "Set up")
                        .id(("acp-registry-setup", index))
                        .flex_none()
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label(format!("Set up {}", agent.name))
                        .tooltip(widgets::text_tooltip(
                            "Install it first, then save its command",
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dialog_open = false;
                            this.edit(Some(custom.clone()), cx);
                            this.editing_id = None;
                        }))
                        .into_any_element()
                }
                None => muted_row("Unavailable").into_any_element(),
            };
            let hover = theme.settings_frame_bg().opacity(0.4);
            rows.push(
                div()
                    .id(("acp-registry-row", index))
                    .h(px(44.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(px(7.))
                    .hover(move |s| s.bg(hover))
                    .child(tile)
                    .child(catalog_ui::title_block(
                        &theme,
                        agent.name.clone(),
                        agent.description.clone(),
                        12.,
                        11.,
                    ))
                    .child(action)
                    .into_any_element(),
            );
        }
        let loading = self.registry.is_none();
        let empty = rows.is_empty();
        let note = |text: String| {
            div()
                .px(px(8.))
                .py(px(16.))
                .text_size(crate::typography::ui_rems(11.))
                .text_color(theme.text_muted)
                .child(text)
        };
        let ghost = |id: &'static str, label: &'static str| {
            let hover = theme.glass_hover();
            div()
                .id(id)
                .h(px(28.))
                .px(px(10.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .text_size(crate::typography::ui_rems(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .cursor_pointer()
                .tab_index(0)
                .role(gpui::Role::Button)
                .hover(move |s| s.bg(hover))
                .child(label)
        };
        let card = div()
            .id("acp-registry-dialog")
            .w(px(448.))
            .rounded(px(popover::CARD_RADIUS))
            .map(|el| popover::glass_surface(&theme, el))
            .overflow_hidden()
            .flex()
            .flex_col()
            .text_color(theme.text)
            .role(gpui::Role::Dialog)
            .aria_label("Add ACP agent")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_dialog(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .p(px(16.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(popover::dialog_title(&theme, "Add ACP agent"))
                    .child(
                        catalog_ui::search_field(&theme, None, self.search.clone().into_any_element())
                            .bg(theme.surface_card)
                            .border_1()
                            .border_color(if focused {
                                theme.accent.opacity(0.7)
                            } else {
                                crate::theme::hairline(0.08)
                            }),
                    ),
            )
            .child(
                div()
                    .id("acp-registry-list")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .px(px(8.))
                    .pb(px(8.))
                    .when(loading, |list| list.child(note("Loading the ACP registry…".into())))
                    .when(!loading && empty, |list| {
                        list.child(note(format!("No agents match “{}”.", query.trim())))
                    })
                    .when_some(
                        self.registry_error.as_ref().filter(|_| !loading),
                        |list, _| {
                            list.child(note(
                                "The ACP registry is unreachable, so only built-in agents are listed."
                                    .into(),
                            ))
                        },
                    )
                    .children(rows),
            )
            .child(
                div()
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(crate::theme::hairline(0.08))
                    .bg(theme.settings_frame_bg().opacity(0.72))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(ghost("acp-custom-command", "Use a custom command…").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.dialog_open = false;
                            this.edit(None, cx);
                        }),
                    ))
                    .child(
                        ghost("acp-registry-done", "Done")
                            .on_click(cx.listener(|this, _, _, cx| this.close_dialog(cx))),
                    ),
            );
        Some(popover::modal_glass(
            "acp-registry-modal",
            window.viewport_size(),
            card.into_any_element(),
            popover::CARD_RADIUS,
        ))
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
        self.registry = Some(registry);
        self.snapshot = Some(CustomAcpAgents {
            revision: 1,
            agents,
        });
        self.dialog_open = dialog;
        cx.notify();
    }
}
impl Render for CustomAcpCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let dialog = self.render_dialog(window, cx);
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
        let editing = self.editing;
        let mut card = widgets::section_card(&theme).mt_0();
        if let Some(error) = &self.error {
            card = card.child(
                div()
                    .px(px(12.))
                    .pt(px(10.))
                    .text_size(crate::typography::ui_rems(11.))
                    .text_color(theme.danger_muted)
                    .child(error.clone()),
            );
        }
        if agents.is_empty() && !editing {
            card = card.child(
                widgets::card_row(&theme, true).child(catalog_ui::title_block(
                    &theme,
                    if self.snapshot.is_none() {
                        "Loading custom agents…"
                    } else {
                        "No custom agents yet"
                    },
                    "Add one from the ACP registry, or run your own command.",
                    12.,
                    11.,
                )),
            );
        }
        let count = agents.len();
        card = card.children(agents.into_iter().enumerate().map(|(index, agent)| {
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
            widgets::card_row(&theme, index == 0)
                .flex_nowrap()
                .gap(px(12.))
                .child(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .rounded(px(7.))
                        .bg(theme.settings_frame_bg().opacity(0.6))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(crate::typography::ui_rems(12.))
                        .text_color(theme.text_muted)
                        .child(initial),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(widgets::row_title(&theme, agent.name.clone()))
                        .child(
                            div()
                                .mt(px(2.))
                                .truncate()
                                .font_family(theme.font_mono.clone())
                                .text_size(crate::typography::ui_rems(10.))
                                .text_color(theme.text_muted)
                                .child(command),
                        ),
                )
                .when(!busy && !editing, |row| {
                    row.child(
                        widgets::ghost_action(&theme)
                            .id(format!("edit-acp-{index}"))
                            .flex_none()
                            .child("Edit")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(Some(agent.clone()), cx)
                            })),
                    )
                    .child(
                        widgets::ghost_action(&theme)
                            .id(format!("delete-acp-{index}"))
                            .flex_none()
                            .child("Remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(snapshot) = &this.snapshot {
                                    let revision = snapshot.revision;
                                    this.request(
                                        methods::DELETE_CUSTOM_ACP_AGENT,
                                        json!({"harness":id,"expectedRevision":revision}),
                                        cx,
                                    );
                                }
                            })),
                    )
                })
        }));
        if editing {
            let mut form = div()
                .px(px(12.))
                .py(px(12.))
                .when(count > 0, |el| {
                    el.border_t_1().border_color(widgets::row_divider(&theme))
                })
                .flex()
                .flex_col()
                .gap(px(12.))
                .text_size(crate::typography::ui_rems(12.))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(11.))
                        .text_color(theme.text_muted)
                        .child("Runs on the selected computer. Arguments have no shell expansion. Use JSON quotes for empty or multiline values. Environment values stay in that computer’s private configuration."),
                );
            for (index, (label, field)) in LABELS.iter().zip(&self.fields).enumerate() {
                if index == 0
                    && let Some(id) = self.editing_id
                {
                    form = form.child(
                        div()
                            .font_family(theme.font_mono.clone())
                            .text_size(crate::typography::ui_rems(11.))
                            .text_color(theme.text_muted)
                            .child(String::from(id)),
                    );
                    continue;
                }
                form = form.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text_muted)
                                .child(*label),
                        )
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(7.))
                                .bg(catalog_ui::control_fill(&theme))
                                .child(field.clone()),
                        ),
                );
            }
            if !busy {
                form = form.child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("cancel-acp".into(), "Cancel").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.editing = false;
                                this.editing_id = None;
                                for field in &this.fields {
                                    field.update(cx, |input, cx| input.set_text("", cx));
                                }
                                cx.notify();
                            },
                        )))
                        .child(
                            catalog_ui::primary_button(&theme, "save-acp")
                                .child("Save agent")
                                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        ),
                );
            }
            card = card.child(form);
        }
        let actions = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                catalog_ui::icon_button(
                    &theme,
                    "refresh-acp",
                    crate::icons::REFRESH,
                    "Refresh custom agents",
                )
                .when(busy, |el| el.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                catalog_ui::secondary_button(&theme, "add-acp")
                    .child(
                        crate::icons::icon(crate::icons::PLUS)
                            .size(px(13.))
                            .text_color(theme.text),
                    )
                    .child("Add ACP agent")
                    .on_click(cx.listener(|this, _, window, cx| this.open_dialog(window, cx))),
            );
        widgets::group_with_action(
            &theme,
            "Custom agents",
            "Agents that speak ACP over standard input and output.",
            Some(actions.into_any_element()),
        )
        .child(card)
        .children(dialog)
    }
}
