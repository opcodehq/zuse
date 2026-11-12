//! Settings → Providers as a list and a detail pane: every agent on the left
//! (set up first, then the ones still to install), the selected one on the
//! right with its Accounts, Models and Settings.
use super::*;
use zuse_protocol::{
    AgentAccountsSnapshot, ClaudeConnection, ClaudeConnectionMode, CustomModel, ProviderModel,
    ProviderModelPrefs, ProxyHealth,
};

/// The selected provider's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum ProviderTab {
    #[default]
    Accounts,
    Models,
    Settings,
}

impl ProviderTab {
    const ALL: [ProviderTab; 3] = [
        ProviderTab::Accounts,
        ProviderTab::Models,
        ProviderTab::Settings,
    ];
    fn label(self) -> &'static str {
        match self {
            ProviderTab::Accounts => "Accounts",
            ProviderTab::Models => "Models",
            ProviderTab::Settings => "Settings",
        }
    }
}

/// Everything the browser keeps beside the page's own state.
pub(super) struct BrowserState {
    pub tab: ProviderTab,
    pub accounts: Option<AgentAccountsSnapshot>,
    pub accounts_task: Option<Task<()>>,
    pub models: std::collections::HashMap<HarnessId, Loadable<Vec<ProviderModel>>>,
    pub models_task: Option<Task<()>>,
    pub model_error: Option<String>,
    pub model_input: Entity<ComposerInput>,
    pub connection: Loadable<ClaudeConnection>,
    pub connection_task: Option<Task<()>>,
    pub connection_error: Option<String>,
    pub connection_busy: bool,
    pub proxy_editing: bool,
    pub proxy_url_input: Entity<ComposerInput>,
    pub proxy_key_input: Entity<ComposerInput>,
    pub _subscriptions: Vec<gpui::Subscription>,
}

impl BrowserState {
    pub fn new(cx: &mut Context<HarnessesPage>) -> Self {
        let model_input = cx.new(|cx| ComposerInput::new("Model ID, e.g. claude-opus-5-5", cx));
        let proxy_url_input = cx.new(|cx| ComposerInput::new("http://127.0.0.1:8317", cx));
        let proxy_key_input = cx.new(|cx| ComposerInput::new("Access key (optional)", cx));
        let subscriptions = vec![
            cx.subscribe(&model_input, |page: &mut HarnessesPage, _, event, cx| {
                if matches!(event, ComposerInputEvent::Submitted) {
                    page.add_model(cx);
                }
            }),
            cx.subscribe(
                &proxy_url_input,
                |page: &mut HarnessesPage, _, event, cx| {
                    if matches!(event, ComposerInputEvent::Submitted) {
                        page.save_proxy(cx);
                    }
                },
            ),
            cx.subscribe(
                &proxy_key_input,
                |page: &mut HarnessesPage, _, event, cx| {
                    if matches!(event, ComposerInputEvent::Submitted) {
                        page.save_proxy(cx);
                    }
                },
            ),
        ];
        Self {
            tab: ProviderTab::default(),
            accounts: None,
            accounts_task: None,
            models: Default::default(),
            models_task: None,
            model_error: None,
            model_input,
            connection: Loadable::Idle,
            connection_task: None,
            connection_error: None,
            connection_busy: false,
            proxy_editing: false,
            proxy_url_input,
            proxy_key_input,
            _subscriptions: subscriptions,
        }
    }
}

const LIST_WIDTH: f32 = 248.0;

impl HarnessesPage {
    /// One engine call whose reply lands back on the page.
    fn browser_call(
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

    pub(super) fn load_browser_accounts(&mut self, cx: &mut Context<Self>) {
        self.browser.accounts_task = self.browser_call(
            methods::LIST_AGENT_ACCOUNTS,
            serde_json::json!({ "forceUsage": false }),
            cx,
            |page, result, _| {
                if let Ok(snapshot) =
                    result.and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
                {
                    page.browser.accounts = Some(snapshot);
                }
            },
        );
    }

    /// Pick the provider shown on the right (never collapses).
    pub(super) fn select_provider(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        self.acp.open = false;
        if self.expanded_harness != Some(harness) {
            self.expanded_harness = Some(harness);
            self.logins.confirm_remove = None;
            self.browser.model_error = None;
            self.browser.connection_error = None;
            self.browser.proxy_editing = false;
        }
        self.ensure_tab_data(cx);
        cx.notify();
    }

    fn select_tab(&mut self, tab: ProviderTab, cx: &mut Context<Self>) {
        self.browser.tab = tab;
        self.ensure_tab_data(cx);
        cx.notify();
    }

    fn ensure_tab_data(&mut self, cx: &mut Context<Self>) {
        let Some(harness) = self.expanded_harness else {
            return;
        };
        if self.browser.tab == ProviderTab::Accounts
            && accounts::signs_in(harness)
            && !self.usage_probed
        {
            self.usage_probed = true;
            self.refresh_usage(cx);
        }
        match self.browser.tab {
            ProviderTab::Models if !self.browser.models.contains_key(&harness) => {
                self.load_models(harness, false, cx)
            }
            ProviderTab::Accounts
                if harness == HarnessId::ClaudeCode
                    && matches!(self.browser.connection, Loadable::Idle) =>
            {
                self.load_connection(cx)
            }
            _ => {}
        }
    }

    /// The first provider worth showing when nothing is picked yet.
    pub(super) fn default_selection(&mut self, cx: &mut Context<Self>) {
        if self.expanded_harness.is_some() {
            return;
        }
        let Loadable::Ready(list) = &self.harnesses else {
            return;
        };
        let first = visible_harnesses(list)
            .into_iter()
            .filter(|d| !matches!(d.id, HarnessId::CustomAcp(_)))
            .find(|d| d.installed && descriptor_enabled(d))
            .map(|d| d.id);
        if let Some(harness) = first {
            self.select_provider(harness, cx);
        }
    }

    // ── models ──────────────────────────────────────────────────────────────

    fn load_models(&mut self, harness: HarnessId, force: bool, cx: &mut Context<Self>) {
        self.browser.models.insert(harness, Loadable::Loading);
        self.browser.models_task = self.browser_call(
            methods::LIST_PROVIDER_MODELS,
            serde_json::json!({ "harness": harness, "force": force }),
            cx,
            move |page, result, _| page.apply_models(harness, result),
        );
    }

    fn apply_models(&mut self, harness: HarnessId, result: Result<serde_json::Value, String>) {
        let parsed = result.and_then(|v| {
            serde_json::from_value::<Vec<ProviderModel>>(v).map_err(|e| e.to_string())
        });
        match parsed {
            Ok(models) => {
                self.browser.models.insert(harness, Loadable::Ready(models));
                self.browser.model_error = None;
            }
            Err(error) => {
                if !matches!(self.browser.models.get(&harness), Some(Loadable::Ready(_))) {
                    self.browser
                        .models
                        .insert(harness, Loadable::Error(error.clone()));
                }
                self.browser.model_error = Some(error);
            }
        }
    }

    fn save_models(
        &mut self,
        harness: HarnessId,
        prefs: ProviderModelPrefs,
        cx: &mut Context<Self>,
    ) {
        self.browser.models_task = self.browser_call(
            methods::SET_PROVIDER_MODELS,
            serde_json::json!({ "harness": harness, "prefs": prefs }),
            cx,
            move |page, result, cx| {
                let ok = result.is_ok();
                page.apply_models(harness, result);
                if ok {
                    page.browser
                        .model_input
                        .update(cx, |input, cx| input.set_text("", cx));
                }
            },
        );
    }

    fn current_prefs(&self, harness: HarnessId) -> Option<ProviderModelPrefs> {
        let Some(Loadable::Ready(models)) = self.browser.models.get(&harness) else {
            return None;
        };
        Some(ProviderModelPrefs {
            hidden: models
                .iter()
                .filter(|m| m.hidden)
                .map(|m| m.id.clone())
                .collect(),
            custom: models
                .iter()
                .filter(|m| m.custom)
                .map(|m| CustomModel {
                    id: m.id.clone(),
                    label: (m.label != m.id).then(|| m.label.clone()),
                })
                .collect(),
        })
    }

    fn toggle_model(&mut self, harness: HarnessId, id: String, cx: &mut Context<Self>) {
        let Some(mut prefs) = self.current_prefs(harness) else {
            return;
        };
        if let Some(at) = prefs.hidden.iter().position(|h| h == &id) {
            prefs.hidden.remove(at);
        } else {
            prefs.hidden.push(id);
        }
        self.save_models(harness, prefs, cx);
    }

    fn remove_model(&mut self, harness: HarnessId, id: String, cx: &mut Context<Self>) {
        let Some(mut prefs) = self.current_prefs(harness) else {
            return;
        };
        prefs.custom.retain(|m| m.id != id);
        prefs.hidden.retain(|h| h != &id);
        self.save_models(harness, prefs, cx);
    }

    fn add_model(&mut self, cx: &mut Context<Self>) {
        let Some(harness) = self.expanded_harness else {
            return;
        };
        let id = self.browser.model_input.read(cx).text().trim().to_owned();
        if id.is_empty() {
            return;
        }
        let Some(mut prefs) = self.current_prefs(harness) else {
            return;
        };
        let known = matches!(self.browser.models.get(&harness), Some(Loadable::Ready(models)) if models.iter().any(|m| m.id == id));
        if known {
            // Already listed: make sure it's shown instead of adding a duplicate.
            prefs.hidden.retain(|h| h != &id);
        } else {
            prefs.custom.push(CustomModel { id, label: None });
        }
        self.save_models(harness, prefs, cx);
    }

    // ── Claude connection ───────────────────────────────────────────────────

    fn load_connection(&mut self, cx: &mut Context<Self>) {
        if matches!(self.browser.connection, Loadable::Idle) {
            self.browser.connection = Loadable::Loading;
        }
        self.browser.connection_task = self.browser_call(
            methods::GET_CLAUDE_CONNECTION,
            serde_json::json!({}),
            cx,
            |page, result, _| page.apply_connection(result, false),
        );
    }

    fn apply_connection(&mut self, result: Result<serde_json::Value, String>, from_edit: bool) {
        self.browser.connection_busy = false;
        match result
            .and_then(|v| serde_json::from_value::<ClaudeConnection>(v).map_err(|e| e.to_string()))
        {
            Ok(connection) => {
                self.browser.connection = Loadable::Ready(connection);
                self.browser.connection_error = None;
                if from_edit {
                    self.browser.proxy_editing = false;
                }
            }
            Err(error) => {
                if matches!(self.browser.connection, Loadable::Loading) {
                    self.browser.connection = Loadable::Error(error.clone());
                }
                self.browser.connection_error = Some(error);
            }
        }
    }

    fn set_connection(
        &mut self,
        params: serde_json::Value,
        from_edit: bool,
        cx: &mut Context<Self>,
    ) {
        self.browser.connection_busy = true;
        self.browser.connection_error = None;
        self.browser.connection_task = self.browser_call(
            methods::SET_CLAUDE_CONNECTION,
            params,
            cx,
            move |page, result, _| {
                let failed_proxy_pick = result.is_err() && !from_edit;
                page.apply_connection(result, from_edit);
                // Nothing found to switch to: ask for the address right here.
                if failed_proxy_pick {
                    page.browser.proxy_editing = true;
                }
            },
        );
        cx.notify();
    }

    fn choose_mode(&mut self, mode: ClaudeConnectionMode, cx: &mut Context<Self>) {
        if self.browser.connection_busy {
            return;
        }
        let current = match &self.browser.connection {
            Loadable::Ready(c) => Some(c.mode),
            _ => None,
        };
        if current == Some(mode) {
            return;
        }
        self.browser.proxy_editing = false;
        self.set_connection(serde_json::json!({ "mode": mode }), false, cx);
    }

    fn save_proxy(&mut self, cx: &mut Context<Self>) {
        let url = self
            .browser
            .proxy_url_input
            .read(cx)
            .text()
            .trim()
            .to_owned();
        let key = self
            .browser
            .proxy_key_input
            .read(cx)
            .text()
            .trim()
            .to_owned();
        let url = if url.is_empty() {
            "http://127.0.0.1:8317".to_owned()
        } else {
            url
        };
        let mut params = serde_json::json!({ "mode": "proxy", "baseUrl": url });
        if !key.is_empty() {
            params["apiKey"] = serde_json::json!(key);
        }
        self.set_connection(params, true, cx);
    }

    fn set_fallback(&mut self, on: bool, cx: &mut Context<Self>) {
        let mode = match &self.browser.connection {
            Loadable::Ready(c) => c.mode,
            _ => return,
        };
        self.set_connection(
            serde_json::json!({ "mode": mode, "fallback": on }),
            false,
            cx,
        );
    }

    // ── rendering ───────────────────────────────────────────────────────────

    pub(super) fn render_browser(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Loadable::Ready(list) = &self.harnesses else {
            return div().into_any_element();
        };
        let descriptors: Vec<HarnessDescriptor> = visible_harnesses(list);
        let (acp, builtin): (Vec<_>, Vec<_>) = descriptors
            .iter()
            .partition(|d| matches!(d.id, HarnessId::CustomAcp(_)));
        let (ready, rest): (Vec<_>, Vec<_>) = builtin
            .into_iter()
            .partition(|d| d.installed && descriptor_enabled(d));
        let (off, missing): (Vec<_>, Vec<_>) = rest.into_iter().partition(|d| d.installed);
        let divider = widgets::row_divider(theme);
        let mut column = div().flex().flex_col().py(px(8.)).gap(px(2.));
        for (label, group) in [
            ("Ready", ready),
            ("ACP agents", acp),
            ("Turned off", off),
            ("Not installed", missing),
        ] {
            if group.is_empty() {
                continue;
            }
            column = column.child(
                div()
                    .px(px(16.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .text_color(theme.text_muted.opacity(0.8))
                    .child(label),
            );
            for descriptor in group {
                column = column.child(self.list_item(descriptor, theme, cx));
            }
        }
        let adding = self.acp.open;
        let active_bg = if theme.variant_id.as_ref() == "zuse-dark" {
            zuse_viewport::settings_style::SIDEBAR_ACTIVE
        } else {
            theme.bg_selected
        };
        column = column.child(
            div()
                .id("provider-add-acp")
                .mx(px(8.))
                .mt(px(8.))
                .px(px(8.))
                .h(px(40.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .gap(px(10.))
                .cursor_pointer()
                .border_1()
                .border_dashed()
                .border_color(theme.border)
                .when(adding, |el| el.bg(active_bg))
                .when(!adding, |el| el.hover(|s| s.bg(active_bg.opacity(0.5))))
                .on_click(cx.listener(|page, _, _, cx| page.open_acp(cx)))
                .child(
                    div()
                        .flex_none()
                        .size(px(28.))
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
                        .text_size(px(12.5))
                        .text_color(theme.text)
                        .child("Add ACP agent"),
                ),
        );
        let selected = self
            .expanded_harness
            .and_then(|id| descriptors.iter().find(|d| d.id == id).cloned());
        let detail = match selected {
            _ if adding => self.render_acp_flow(theme, cx),
            Some(descriptor) => self.detail(&descriptor, theme, cx),
            None => div()
                .p(px(32.))
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child("Pick a provider to see its accounts, models and settings.")
                .into_any_element(),
        };
        widgets::section_card(theme)
            .mt_0()
            .flex_row()
            .items_stretch()
            .min_h(px(560.))
            .child(
                div()
                    .w(px(LIST_WIDTH))
                    .flex_none()
                    .border_r_1()
                    .border_color(divider)
                    .bg(theme.settings_frame_bg().opacity(0.25))
                    .child(column),
            )
            .child(div().flex_1().min_w_0().child(detail))
            .into_any_element()
    }

    fn active_account(&self, harness: HarnessId) -> Option<&zuse_protocol::AgentAccount> {
        self.browser
            .accounts
            .as_ref()?
            .accounts
            .iter()
            .find(|a| a.harness == harness && a.active)
    }

    fn account_count(&self, harness: HarnessId) -> usize {
        self.browser.accounts.as_ref().map_or(0, |s| {
            s.accounts.iter().filter(|a| a.harness == harness).count()
        })
    }

    fn list_item(
        &self,
        d: &HarnessDescriptor,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let harness = d.id;
        let selected = !self.acp.open && self.expanded_harness == Some(harness);
        let enabled = d.installed && descriptor_enabled(d);
        let (icon_path, tint) = crate::pickers::harness_brand_icon(harness);
        let custom = matches!(harness, HarnessId::CustomAcp(_));
        let subtitle: SharedString = if custom && enabled {
            match self.acp.checks.get(&harness) {
                Some(Err(_)) => "Not answering".into(),
                _ => "ACP agent".into(),
            }
        } else if !d.installed {
            "Not installed".into()
        } else if !enabled {
            "Off".into()
        } else if harness == HarnessId::ClaudeCode
            && matches!(&self.browser.connection, Loadable::Ready(c) if c.mode == ClaudeConnectionMode::Proxy)
        {
            "Through CLI proxy".into()
        } else if let Some(account) = self.active_account(harness) {
            let who = account
                .email
                .as_deref()
                .map(zuse_protocol::presentation::masked_email)
                .unwrap_or_else(|| "Signed in".into());
            match self.account_count(harness) {
                n if n > 1 => format!("{who} · {n} accounts").into(),
                _ => who.into(),
            }
        } else if d.signed_in == Some(false) {
            "Sign in to use it".into()
        } else {
            "Ready".into()
        };
        // Only agents that can't run without a login warn; the engine says
        // which (an API key or the Claude proxy counts as signed in).
        let dot = if !enabled {
            None
        } else if d.signed_in == Some(false) {
            Some(theme.warning)
        } else {
            Some(theme.success)
        };
        let active_bg = if theme.variant_id.as_ref() == "zuse-dark" {
            zuse_viewport::settings_style::SIDEBAR_ACTIVE
        } else {
            theme.bg_selected
        };
        div()
            .id(SharedString::from(format!("provider-item-{harness:?}")))
            .mx(px(8.))
            .px(px(8.))
            .h(px(44.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .role(gpui::Role::Tab)
            .aria_selected(selected)
            .tab_index(0)
            .when(selected, |el| el.bg(active_bg))
            .when(!selected, |el| el.hover(|s| s.bg(active_bg.opacity(0.5))))
            .on_click(cx.listener(move |page, _, _, cx| page.select_provider(harness, cx)))
            .child(
                div()
                    .flex_none()
                    .size(px(28.))
                    .rounded(px(7.))
                    .bg(theme.settings_frame_bg().opacity(0.8))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(!enabled, |el| el.opacity(0.55))
                    .child(
                        crate::icons::icon(icon_path)
                            .size(px(16.))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if enabled {
                                theme.text_strong
                            } else {
                                theme.text_muted
                            })
                            .child(d.name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(subtitle),
                    ),
            )
            .when_some(dot, |el, color| {
                el.child(div().flex_none().size(px(6.)).rounded_full().bg(color))
            })
            .into_any_element()
    }

    fn detail(&self, d: &HarnessDescriptor, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let harness = d.id;
        let enabled = match self.pending_toggle {
            Some((pending, on)) if pending == harness => on,
            _ => descriptor_enabled(d),
        };
        let (icon_path, tint) = crate::pickers::harness_brand_icon(harness);
        let update = match &self.updates {
            Loadable::Ready(statuses) => statuses.iter().find(|s| s.harness == harness),
            _ => None,
        };
        let version = update
            .filter(|_| d.installed)
            .and_then(|s| s.installed_version.clone())
            .map(|v| SharedString::from(format!("v{v}")));
        let (tone, status) = provider_status(harness, d.installed, enabled, d.can_install, update);
        let can_update = update.is_some_and(|s| {
            s.can_apply
                && matches!(
                    s.phase,
                    HarnessUpdatePhase::Available | HarnessUpdatePhase::ManualActionRequired
                )
        });
        let latest = update.and_then(|s| s.latest_version.clone());
        let updating = update.is_some_and(|s| {
            matches!(
                s.phase,
                HarnessUpdatePhase::WaitingForIdle
                    | HarnessUpdatePhase::Preparing
                    | HarnessUpdatePhase::Downloading
            )
        });
        let installing = self.installing == Some(harness);
        let custom_missing = matches!(harness, HarnessId::CustomAcp(_)) && !d.installed;
        let status = if installing {
            install_label(&d.name)
        } else if custom_missing {
            "Its command isn't found on this computer".into()
        } else if d.installed && enabled && d.signed_in == Some(false) {
            "Not signed in".into()
        } else {
            status
        };
        let enabled_count = match &self.harnesses {
            Loadable::Ready(list) => list.iter().filter(|d| descriptor_enabled(d)).count(),
            _ => 0,
        };
        let interactive =
            !(enabled && enabled_count == 1 && d.installed) && (enabled || d.installed);

        let header = div()
            .px(px(24.))
            .pt(px(20.))
            .pb(px(16.))
            .flex()
            .items_center()
            .gap(px(14.))
            .child(
                div()
                    .flex_none()
                    .size(px(44.))
                    .rounded(px(11.))
                    .bg(theme.settings_frame_bg().opacity(0.9))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::icons::icon(icon_path)
                            .size(px(24.))
                            .text_color(tint.unwrap_or(theme.text)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text_strong)
                                    .child(d.name.clone()),
                            )
                            .when_some(version, |el, v| {
                                el.child(
                                    div()
                                        .font_family(theme.font_mono.clone())
                                        .text_size(px(11.))
                                        .text_color(theme.text_muted)
                                        .child(v),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child(div().size(px(6.)).rounded_full().bg(
                                if custom_missing
                                    || (d.installed && enabled && d.signed_in == Some(false))
                                {
                                    theme.warning
                                } else {
                                    tone.color(theme)
                                },
                            ))
                            .child(SharedString::from(status)),
                    ),
            )
            .when(can_update, |el| {
                el.child(
                    widgets::text_action(
                        theme,
                        widgets::ActionTone::Outlined,
                        latest.map_or_else(|| "Update".to_owned(), |v| format!("Update to v{v}")),
                    )
                    .id("provider-detail-update")
                    .on_click(
                        cx.listener(move |page, _, _, cx| page.apply_harness_update(harness, cx)),
                    ),
                )
            })
            .when(updating, |el| {
                el.child(
                    widgets::text_action(theme, widgets::ActionTone::Quiet, "Cancel update")
                        .id("provider-detail-cancel-update")
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.cancel_harness_update(harness, cx)
                        })),
                )
            })
            .when(
                offers_install(harness, d.installed, d.can_install) || installing,
                |el| {
                    el.child(
                        widgets::text_action(
                            theme,
                            widgets::ActionTone::Filled,
                            if installing {
                                "Cancel install"
                            } else {
                                "Install"
                            },
                        )
                        .id("provider-detail-install")
                        .on_click(cx.listener(move |page, _, _, cx| {
                            if installing {
                                page.cancel_install(cx)
                            } else {
                                page.install(harness, cx)
                            }
                        })),
                    )
                },
            )
            .when(d.installed, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.text_muted)
                                .child(if enabled { "On" } else { "Off" }),
                        )
                        .child(
                            widgets::toggle_switch(
                                theme,
                                enabled,
                                format!("provider-detail-{harness:?}"),
                            )
                            .id("provider-detail-toggle")
                            .role(gpui::Role::Switch)
                            .aria_label(d.name.clone())
                            .when(!interactive, |el| el.opacity(0.5))
                            .when(interactive, |el| {
                                el.cursor_pointer()
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.toggle(harness, !enabled, cx)
                                    }))
                            }),
                        ),
                )
            });

        let custom = matches!(harness, HarnessId::CustomAcp(_));
        if !d.installed && !custom {
            let command = zuse_providers::install::manual_command(harness);
            let body = div()
                .px(px(24.))
                .pb(px(24.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(div().text_size(px(12.)).text_color(theme.text_muted).child(
                    if d.can_install {
                        "Zuse can install it for you, or run this yourself and refresh."
                    } else {
                        "Run this on this computer, then refresh."
                    },
                ))
                .when_some(command, |el, command| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(px(8.))
                            .bg(theme.settings_frame_bg().opacity(0.6))
                            .px(px(12.))
                            .py(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_family(theme.font_mono.clone())
                                    .text_size(px(11.5))
                                    .child(command),
                            )
                            .child(
                                widgets::text_action(theme, widgets::ActionTone::Quiet, "Copy")
                                    .id("provider-copy-install")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            command.to_owned(),
                                        ))
                                    }),
                            ),
                    )
                });
            return div()
                .flex()
                .flex_col()
                .child(header)
                .child(body)
                .into_any_element();
        }

        let tabs = div()
            .px(px(20.))
            .flex()
            .items_end()
            .gap(px(4.))
            .border_b_1()
            .border_color(widgets::row_divider(theme))
            .children(ProviderTab::ALL.into_iter().map(|tab| {
                let selected = self.browser.tab == tab;
                let label = match (tab, harness) {
                    (ProviderTab::Accounts, HarnessId::CustomAcp(_)) => "Setup",
                    _ => tab.label(),
                };
                div()
                    .id(SharedString::from(format!("provider-tab-{}", tab.label())))
                    .px(px(10.))
                    .pt(px(6.))
                    .pb(px(9.))
                    .cursor_pointer()
                    .role(gpui::Role::Tab)
                    .aria_selected(selected)
                    .tab_index(0)
                    .text_size(px(12.5))
                    .font_weight(if selected {
                        gpui::FontWeight::MEDIUM
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(if selected {
                        theme.text_strong
                    } else {
                        theme.text_muted
                    })
                    .border_b_2()
                    .border_color(if selected {
                        theme.text_strong
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(|s| s.text_color(theme.text))
                    .on_click(cx.listener(move |page, _, _, cx| page.select_tab(tab, cx)))
                    .child(label)
            }));

        let body: AnyElement = if custom && !d.installed {
            // Its command isn't found: the Setup tab says why and offers Edit.
            self.acp_agent_tab(harness, theme, cx)
        } else if !enabled {
            div()
                .p(px(24.))
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child(format!(
                    "{} is turned off. Turn it on to use it in chats.",
                    d.name
                ))
                .into_any_element()
        } else {
            match self.browser.tab {
                ProviderTab::Accounts if matches!(harness, HarnessId::CustomAcp(_)) => {
                    self.acp_agent_tab(harness, theme, cx)
                }
                ProviderTab::Accounts => self.accounts_tab(harness, theme, cx),
                ProviderTab::Models => self.models_tab(harness, theme, cx),
                ProviderTab::Settings => div()
                    .p(px(24.))
                    .flex()
                    .flex_col()
                    .gap(px(20.))
                    .child(self.render_completion_for(harness, theme, cx))
                    .children(self.render_updates_for(harness, theme, cx))
                    .into_any_element(),
            }
        };
        div()
            .flex()
            .flex_col()
            .child(header)
            .when(enabled && (d.installed || !custom), |el| el.child(tabs))
            .child(body)
            .into_any_element()
    }

    fn block_title(theme: &Theme, title: &str, description: &str) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_strong)
                    .child(SharedString::from(title.to_owned())),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(description.to_owned())),
            )
    }

    fn accounts_tab(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let proxied = harness == HarnessId::ClaudeCode
            && matches!(&self.browser.connection, Loadable::Ready(c) if c.mode == ClaudeConnectionMode::Proxy);
        div()
            .px(px(24.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(18.))
            .when(harness == HarnessId::ClaudeCode, |el| el.child(self.connection_block(theme, cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .when(proxied, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .rounded(px(8.))
                                .bg(theme.settings_frame_bg().opacity(0.5))
                                .px(px(12.))
                                .py(px(8.))
                                .text_size(px(11.5))
                                .text_color(theme.text_muted)
                                .child(crate::icons::icon(crate::icons::INFO_CIRCLE).size(px(14.)).text_color(theme.text_muted))
                                .child("These accounts aren't used while Claude runs through the CLI proxy."),
                        )
                    })
                    .child(if accounts::signs_in(harness) {
                        div()
                            .when(proxied, |el| el.opacity(0.55))
                            .child(self.accounts_panel(harness, theme, cx))
                            .into_any_element()
                    } else {
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child("This agent signs in on its own; there's nothing to manage here.")
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn segmented(
        theme: &Theme,
        id: &'static str,
        options: &[(&'static str, bool)],
    ) -> Vec<gpui::Stateful<gpui::Div>> {
        options
            .iter()
            .enumerate()
            .map(|(ix, (label, selected))| {
                div()
                    .id((id, ix))
                    .px(px(12.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(*selected, |el| {
                        el.bg(theme.settings_frame_bg())
                            .text_color(theme.text_strong)
                            .font_weight(gpui::FontWeight::MEDIUM)
                    })
                    .when(!*selected, |el| {
                        el.text_color(theme.text_muted)
                            .hover(|s| s.text_color(theme.text))
                    })
                    .child(*label)
            })
            .collect()
    }

    fn connection_block(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let connection = match &self.browser.connection {
            Loadable::Ready(c) => Some(c.clone()),
            _ => None,
        };
        let proxy = connection
            .as_ref()
            .is_some_and(|c| c.mode == ClaudeConnectionMode::Proxy);
        let mut options = Self::segmented(
            theme,
            "claude-connection",
            &[("Claude account", !proxy), ("CLI proxy", proxy)],
        )
        .into_iter();
        let account_tab = options.next().unwrap().on_click(
            cx.listener(|page, _, _, cx| page.choose_mode(ClaudeConnectionMode::Account, cx)),
        );
        let proxy_tab = options.next().unwrap().on_click(
            cx.listener(|page, _, _, cx| page.choose_mode(ClaudeConnectionMode::Proxy, cx)),
        );
        let segmented = div()
            .flex_none()
            .flex()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(8.))
            .bg(theme.settings_frame_bg().opacity(0.5))
            .when(self.browser.connection_busy, |el| el.opacity(0.6))
            .child(account_tab)
            .child(proxy_tab);

        let mut block = div().flex().flex_col().gap(px(12.)).child(
            div()
                .flex()
                .items_start()
                .gap(px(16.))
                .child(Self::block_title(
                    theme,
                    "Connection",
                    "Use your Claude account directly, or a CLI proxy you run that pools several subscriptions.",
                ).flex_1().min_w_0())
                .child(segmented),
        );

        if proxy || self.browser.proxy_editing {
            let mut card = widgets::section_card(theme).mt_0();
            if let Some(c) = connection
                .as_ref()
                .filter(|c| c.mode == ClaudeConnectionMode::Proxy)
            {
                let (color, line): (gpui::Hsla, String) = match c.health {
                    Some(ProxyHealth::Connected) => (
                        theme.success,
                        format!("Connected · {}", c.base_url.clone().unwrap_or_default()),
                    ),
                    Some(ProxyHealth::Unreachable) => (
                        theme.danger,
                        c.message
                            .clone()
                            .unwrap_or_else(|| "The proxy isn't answering.".into()),
                    ),
                    Some(ProxyHealth::Rejected) => (
                        theme.warning,
                        c.message
                            .clone()
                            .unwrap_or_else(|| "The proxy refused Zuse.".into()),
                    ),
                    None => (theme.text_muted, c.base_url.clone().unwrap_or_default()),
                };
                card = card.child(
                    widgets::card_row(theme, true)
                        .child(div().flex_none().size(px(7.)).rounded_full().bg(color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(widgets::row_title(theme, line))
                                .child(
                                    div().text_size(px(11.)).text_color(theme.text_muted).child(
                                        if c.has_key {
                                            "Access key saved"
                                        } else {
                                            "No access key"
                                        },
                                    ),
                                ),
                        )
                        .child(
                            widgets::text_action(theme, widgets::ActionTone::Quiet, "Check")
                                .id("proxy-check")
                                .on_click(cx.listener(|page, _, _, cx| page.load_connection(cx))),
                        )
                        .when(!self.browser.proxy_editing, |el| {
                            el.child(
                                widgets::text_action(theme, widgets::ActionTone::Outlined, "Edit")
                                    .id("proxy-edit")
                                    .on_click(cx.listener(|page, _, _, cx| {
                                        let url = match &page.browser.connection {
                                            Loadable::Ready(c) => {
                                                c.base_url.clone().unwrap_or_default()
                                            }
                                            _ => String::new(),
                                        };
                                        page.browser
                                            .proxy_url_input
                                            .update(cx, |i, cx| i.set_text(url, cx));
                                        page.browser
                                            .proxy_key_input
                                            .update(cx, |i, cx| i.set_text("", cx));
                                        page.browser.proxy_editing = true;
                                        cx.notify();
                                    })),
                            )
                        }),
                );
                card = card.child(
                    widgets::card_row(theme, false)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(widgets::row_title(
                                    theme,
                                    "Use my Claude account if the proxy is down",
                                ))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.text_muted)
                                        .child("Chats start on your account instead of failing."),
                                ),
                        )
                        .child(
                            widgets::toggle_switch(theme, c.fallback, "proxy-fallback")
                                .id("proxy-fallback")
                                .cursor_pointer()
                                .role(gpui::Role::Switch)
                                .on_click({
                                    let on = !c.fallback;
                                    cx.listener(move |page, _, _, cx| page.set_fallback(on, cx))
                                }),
                        ),
                );
            }
            if self.browser.proxy_editing {
                card = card.child(
                    widgets::card_row(
                        theme,
                        connection
                            .as_ref()
                            .is_none_or(|c| c.mode != ClaudeConnectionMode::Proxy),
                    )
                    .flex_col()
                    .items_stretch()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(widgets::field_label(theme, "Address"))
                                    .child(popover::dialog_field(
                                        self.browser.proxy_url_input.clone().into_any_element(),
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(widgets::field_label(theme, "Access key"))
                                    .child(popover::dialog_field(
                                        self.browser.proxy_key_input.clone().into_any_element(),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                widgets::text_action(theme, widgets::ActionTone::Quiet, "Cancel")
                                    .id("proxy-cancel")
                                    .on_click(cx.listener(|page, _, _, cx| {
                                        page.browser.proxy_editing = false;
                                        page.browser.connection_error = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                widgets::text_action(
                                    theme,
                                    widgets::ActionTone::Filled,
                                    if self.browser.connection_busy {
                                        "Checking…"
                                    } else {
                                        "Save and use"
                                    },
                                )
                                .id("proxy-save")
                                .on_click(cx.listener(|page, _, _, cx| page.save_proxy(cx))),
                            ),
                    ),
                );
            }
            block = block.child(card);
        }
        if let Some(error) = &self.browser.connection_error {
            block = block.child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.danger_muted)
                    .child(SharedString::from(error.clone())),
            );
        }
        block.into_any_element()
    }

    fn models_tab(&self, harness: HarnessId, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let models = self.browser.models.get(&harness);
        let add = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(theme.font_mono.clone())
                    .child(popover::dialog_field(
                        self.browser.model_input.clone().into_any_element(),
                    )),
            )
            .child(
                widgets::action_button(theme, widgets::ActionTone::Outlined)
                    .id("provider-add-model")
                    .child(
                        crate::icons::icon(crate::icons::PLUS)
                            .size(px(12.))
                            .text_color(theme.text),
                    )
                    .child("Add model")
                    .on_click(cx.listener(|page, _, _, cx| page.add_model(cx))),
            );
        let list: AnyElement = match models {
            None | Some(Loadable::Idle | Loadable::Loading) => widgets::section_card(theme)
                .mt_0()
                .p(px(16.))
                .child(popover::skeleton_rows(
                    "provider-models-skeleton",
                    theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Some(Loadable::Error(error)) => div()
                .text_size(px(12.))
                .text_color(theme.danger_muted)
                .child(SharedString::from(format!("Couldn't load models: {error}")))
                .into_any_element(),
            Some(Loadable::Ready(models)) if models.is_empty() => div()
                .text_size(px(12.))
                .text_color(theme.text_muted)
                .child("This agent reported no models. Add a model ID above.")
                .into_any_element(),
            Some(Loadable::Ready(models)) => widgets::section_card(theme)
                .mt_0()
                .children(
                    models
                        .iter()
                        .filter(|m| m.custom)
                        .chain(models.iter().filter(|m| !m.custom))
                        .enumerate()
                        .map(|(ix, model)| {
                            let id = model.id.clone();
                            let remove_id = model.id.clone();
                            let shown = !model.hidden;
                            widgets::card_row(theme, ix == 0)
                                .flex_nowrap()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .child(
                                                    widgets::row_title(theme, model.label.clone())
                                                        .when(!shown, |el| {
                                                            el.text_color(theme.text_muted)
                                                        }),
                                                )
                                                .when(model.custom, |el| {
                                                    el.child(widgets::badge(theme, "Added"))
                                                }),
                                        )
                                        .when(model.label != model.id, |el| {
                                            el.child(
                                                div()
                                                    .truncate()
                                                    .font_family(theme.font_mono.clone())
                                                    .text_size(px(11.))
                                                    .text_color(theme.text_muted)
                                                    .child(model.id.clone()),
                                            )
                                        }),
                                )
                                .when(model.custom, |el| {
                                    el.child(
                                        div()
                                            .id(("provider-remove-model", ix))
                                            .size(px(26.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(6.))
                                            .cursor_pointer()
                                            .role(gpui::Role::Button)
                                            .aria_label("Remove model")
                                            .hover(|s| s.bg(theme.settings_frame_bg()))
                                            .on_click(cx.listener(move |page, _, _, cx| {
                                                page.remove_model(harness, remove_id.clone(), cx)
                                            }))
                                            .child(
                                                crate::icons::icon(
                                                    crate::icons::TRASH_BIN_MINIMALISTIC,
                                                )
                                                .size(px(14.))
                                                .text_color(theme.text_muted),
                                            ),
                                    )
                                })
                                .child(
                                    widgets::toggle_switch(
                                        theme,
                                        shown,
                                        format!("model-{harness:?}-{}", model.id),
                                    )
                                    .id(("provider-model-toggle", ix))
                                    .cursor_pointer()
                                    .role(gpui::Role::Switch)
                                    .aria_label(format!("Show {} in the model picker", model.label))
                                    .on_click(cx.listener(
                                        move |page, _, _, cx| {
                                            page.toggle_model(harness, id.clone(), cx)
                                        },
                                    )),
                                )
                        }),
                )
                .into_any_element(),
        };
        let summary = match models {
            Some(Loadable::Ready(models)) => {
                let hidden = models.iter().filter(|m| m.hidden).count();
                let shown = models.len() - hidden;
                if hidden > 0 {
                    format!("{shown} shown in the picker · {hidden} hidden")
                } else {
                    format!("{shown} shown in the picker")
                }
            }
            _ => String::new(),
        };
        div()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .child(Self::block_title(
                        theme,
                        "Models",
                        "Choose which models appear in the model picker. Add a model ID the agent doesn't list yet, like a new release or a proxy's model name.",
                    ).flex_1().min_w_0())
                    .child(
                        widgets::text_action(theme, widgets::ActionTone::Quiet, "Refresh")
                            .id("provider-models-refresh")
                            .on_click(cx.listener(move |page, _, _, cx| page.load_models(harness, true, cx))),
                    ),
            )
            .child(add)
            .when_some(self.browser.model_error.clone(), |el, error| {
                el.child(div().text_size(px(11.5)).text_color(theme.danger_muted).child(SharedString::from(error)))
            })
            .when(!summary.is_empty(), |el| {
                el.child(div().text_size(px(11.)).text_color(theme.text_muted).child(SharedString::from(summary)))
            })
            .child(list)
            .into_any_element()
    }
}
