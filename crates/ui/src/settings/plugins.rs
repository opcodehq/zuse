//! Account and hosted connections, backed by the engine's shared authority.
//!
//! One [`PluginsCard`] per app (see [`PluginsCard::shared`]) owns the account
//! stream, the plugin snapshot and every plugin request. Settings → Plugins
//! renders it as the installed list; [`PluginsPage`] renders the same entity
//! as the browse catalog in the main area.
use crate::{composer::ComposerInput, popover, settings::widgets, state::AppState, theme::Theme};
use gpui::{
    AnyElement, App, Context, Entity, Global, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use serde_json::{Value, json};
use zuse_engine::account::AccountStatus;
use zuse_rpc::methods;
use zuse_viewport::plugin_setup::{PluginSetup, SetupColors, SetupEvent, SetupTarget};

#[path = "catalog_ui.rs"]
pub(crate) mod catalog_ui;

gpui::actions!(
    zuse_plugins,
    [
        /// Show the plugin catalog page in the main area.
        OpenPluginsPage
    ]
);

/// How many featured plugins the browse page lists before "All plugins".
const FEATURED_LIMIT: usize = 12;
/// The plugin Workspace (Personal / organization) picker. Organization
/// features are hidden for now; the selection logic stays for their return.
const SHOW_PLUGIN_WORKSPACES: bool = false;

pub struct PluginsCard {
    state: Entity<AppState>,
    account_only: bool,
    /// The browse page's search.
    search: Entity<ComposerInput>,
    /// Settings → Plugins' search over connected plugins.
    installed_search: Entity<ComposerInput>,
    /// The browse page shows only connected plugins ("Connected" tab).
    connected_only: bool,
    browse_selected: Option<String>,
    detail_menu: zuse_viewport::plugin_detail::Menu,
    add_menu_open: bool,
    /// When the Add menu last closed from an outside press, so the trigger's
    /// own click doesn't reopen it.
    add_menu_closed: Option<std::time::Instant>,
    _search: Subscription,
    _installed_search: Subscription,
    account: AccountStatus,
    account_loaded: bool,
    account_view: Option<Entity<zuse_viewport::account_settings::AccountCard>>,
    account_view_events: Option<Subscription>,
    plugins: zuse_protocol::plugin_state::PluginState,
    error: Option<String>,
    epoch: u64,
    busy: bool,
    watch: Option<Task<()>>,
    action: Option<Task<()>>,
    poll: Option<Task<()>>,
    setup: Option<Entity<PluginSetup>>,
    setup_events: Option<Subscription>,
    /// Review fixtures seed the account and snapshot without an engine.
    fixture: bool,
    _state: Subscription,
}

struct SharedPlugins(Entity<PluginsCard>);
impl Global for SharedPlugins {}

impl PluginsCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.fixture {
                return;
            }
            if this.state.read(cx).engine().is_none() {
                this.watch = None;
                this.action = None;
                this.busy = false;
                this.epoch = this.epoch.wrapping_add(1);
                this.setup = None;
                this.setup_events = None;
                this.account = AccountStatus::default();
                this.account_loaded = false;
                this.plugins = Default::default();
            } else if this.watch.is_none() {
                this.attach(cx);
            }
        });
        let search = cx.new(|cx| {
            ComposerInput::new("Search plugins", cx)
                .with_single_line()
                .with_tab_stop()
        });
        let installed_search = cx.new(|cx| {
            ComposerInput::new("Search", cx)
                .with_single_line()
                .with_tab_stop()
        });
        let search_events = cx.observe(&search, |_: &mut Self, _, cx| cx.notify());
        let installed_events = cx.observe(&installed_search, |_: &mut Self, _, cx| cx.notify());
        let mut card = Self {
            state,
            account_only: false,
            search,
            installed_search,
            connected_only: false,
            browse_selected: None,
            detail_menu: Default::default(),
            add_menu_open: false,
            add_menu_closed: None,
            _search: search_events,
            _installed_search: installed_events,
            account: AccountStatus::default(),
            account_loaded: false,
            account_view: None,
            account_view_events: None,
            plugins: Default::default(),
            error: None,
            epoch: 0,
            busy: false,
            watch: None,
            action: None,
            poll: None,
            setup: None,
            setup_events: None,
            fixture: false,
            _state: subscription,
        };
        card.attach(cx);
        card
    }
    /// The app's one plugins entity, shared by Settings and the browse page
    /// so a connection made on one shows on the other without a reload.
    pub fn shared(state: Entity<AppState>, cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedPlugins>() {
            return shared.0.clone();
        }
        let card = cx.new(|cx| Self::new(state, cx));
        cx.set_global(SharedPlugins(card.clone()));
        card
    }
    pub fn account(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut card = Self::new(state, cx);
        card.account_only = true;
        card
    }
    fn attach(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.watch = Some(cx.spawn(async move |this, cx| {
            let stream = engine
                .client()
                .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await;
            match stream {
                Ok(mut stream) => {
                    while let Some(value) = stream.recv().await {
                        let Ok(account) = serde_json::from_value::<AccountStatus>(value) else {
                            continue;
                        };
                        if this
                            .update(cx, |card, cx| {
                                let changed = card.account.user.as_ref().map(|u| &u.id)
                                    != account.user.as_ref().map(|u| &u.id);
                                card.account = account;
                                card.account_loaded = true;
                                if changed {
                                    card.browse_selected = None;
                                    card.detail_menu = Default::default();
                                    card.setup = None;
                                    card.setup_events = None;
                                    card.epoch = card.epoch.wrapping_add(1);
                                    card.plugins = Default::default();
                                    card.action = None;
                                    card.busy = false;
                                    if card.account.user.is_some() && !card.account_only {
                                        card.request(
                                            methods::PLUGINS,
                                            json!({"action":"list"}),
                                            cx,
                                        );
                                    }
                                }
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = this.update(cx, |card, cx| {
                        card.error = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        }));
    }
    fn open_setup(&mut self, target: SetupTarget, window: &mut Window, cx: &mut Context<Self>) {
        if self.plugins.active_attempt() {
            self.error = Some("Finish or cancel the current plugin sign-in first.".into());
            cx.notify();
            return;
        }
        if self.busy || self.account.user.is_none() || self.plugins.snapshot["canManage"] == false {
            return;
        }
        let Some(tenant) = self.plugins.snapshot["tenantId"]
            .as_str()
            .map(str::to_owned)
        else {
            return;
        };
        let expected = tenant.clone();
        let epoch = self.epoch;
        let theme = Theme::of(cx).for_settings_surface();
        let colors = SetupColors {
            text: theme.text,
            muted: theme.text_muted,
            background: zuse_viewport::palette::rgb(0x191c1a).into(),
            border: theme.border,
            danger: theme.danger,
        };
        let form = cx.new(|cx| {
            PluginSetup::new(
                tenant,
                target,
                zuse_protocol::plugins::PluginReturnTo::Desktop { port: 0 },
                colors,
                window,
                cx,
            )
        });
        self.setup_events = Some(cx.subscribe(&form, move |this, _, event, cx| {
            match event {
                SetupEvent::Cancel => {
                    this.setup = None;
                    this.setup_events = None;
                }
                SetupEvent::Submit(request) => {
                    if this.busy
                        || this.epoch != epoch
                        || this.account.user.is_none()
                        || this.plugins.snapshot["tenantId"] != expected
                        || this.plugins.snapshot["canManage"] == false
                    {
                        return;
                    }
                    this.setup = None;
                    this.setup_events = None;
                    if let Ok(value) = serde_json::to_value(request) {
                        this.request(methods::PLUGINS, value, cx);
                    }
                }
            }
            cx.notify();
        }));
        self.setup = Some(form);
        cx.notify();
    }
    fn ask_removal(
        &mut self,
        kind: zuse_protocol::plugin_state::Removal,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).engine().is_none() {
            return;
        }
        if let Err(error) = self.plugins.ask_removal(kind, id) {
            self.error = Some(error.into());
        }
        cx.notify();
    }
    fn confirm_removal(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).engine().is_none() {
            return;
        }
        match self.plugins.confirm_removal() {
            Ok(request) => self.request(methods::PLUGINS, request, cx),
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }
    fn request(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = if matches!(method, methods::PLUGINS | methods::SELECT_PLUGIN_WORKSPACE) {
            let prepared = if method == methods::SELECT_PLUGIN_WORKSPACE {
                self.plugins.begin_workspace_selection(params)
            } else {
                self.plugins.begin(params)
            };
            match prepared {
                Ok(request) => request,
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            }
        } else {
            params
        };
        self.setup = None;
        self.setup_events = None;
        self.busy = true;
        self.error = None;
        let epoch = self.epoch;
        self.action = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                card.busy = false;
                match result {
                    Ok(value)
                        if matches!(
                            method,
                            methods::PLUGINS | methods::SELECT_PLUGIN_WORKSPACE
                        ) =>
                    {
                        let next = match card.plugins.apply(&value) {
                            Ok(next) => next,
                            Err(error) => {
                                card.error = Some(error.into());
                                card.plugins.failed()
                            }
                        };
                        if let Some(next) = next {
                            card.request(methods::PLUGINS, next, cx);
                        }
                    }
                    Ok(value) if method == methods::CLOUD_ACCOUNT_SIGN_IN => {
                        if let Some(url) = value["url"]
                            .as_str()
                            .filter(|url| url.starts_with("https://"))
                        {
                            cx.open_url(url);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        card.error = Some(error.to_string());
                        if matches!(method, methods::PLUGINS | methods::SELECT_PLUGIN_WORKSPACE) {
                            if let Some(next) = card.plugins.failed() {
                                card.request(methods::PLUGINS, next, cx);
                            }
                        }
                    }
                }
                card.schedule_poll(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn schedule_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        if !self.plugins.active_attempt() || self.busy {
            return;
        }
        let epoch = self.epoch;
        let id = self.plugins.attempt["id"].clone();
        let tenant = self.plugins.snapshot["tenantId"].clone();
        self.poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.epoch == epoch
                    && this.signed_in()
                    && !this.busy
                    && this.plugins.active_attempt()
                    && this.plugins.attempt["id"] == id
                    && this.plugins.snapshot["tenantId"] == tenant
                {
                    this.request(
                        methods::PLUGINS,
                        json!({"action":"poll","tenantId":tenant,"attemptId":id}),
                        cx,
                    );
                }
            });
        }));
    }
}

/// One connection from the snapshot, joined with its catalog entry.
struct Connection {
    id: Value,
    label: String,
    description: String,
    domain: String,
    state: String,
    enabled: bool,
}

impl PluginsCard {
    fn signed_in(&self) -> bool {
        self.account.user.is_some()
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        self.request(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}), cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.signed_in() {
            self.request(methods::PLUGINS, json!({"action":"list"}), cx);
        }
    }

    /// Connect a catalog plugin: signed out, sign in first; plugins keyed by
    /// an API key collect it in the setup form; the rest start hosted OAuth.
    fn connect(
        &mut self,
        definition: zuse_protocol::plugins::PluginDefinition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zuse_viewport::plugin_catalog as catalog;
        if self.busy || self.plugins.snapshot["canManage"] == false {
            return;
        }
        let label = catalog::next_label(&self.plugins.snapshot, &definition);
        if !self.signed_in() {
            self.sign_in(cx);
            return;
        }
        if definition.api_key_header.is_some() {
            self.detail_menu
                .connect_focus
                .get_or_insert_with(|| cx.focus_handle());
            self.browse_selected = Some(definition.id.clone());
            self.open_setup(SetupTarget::Catalog(definition), window, cx);
            if let Some(form) = &self.setup {
                form.update(cx, |form, cx| {
                    form.set_connection_label(label, window, cx);
                    form.set_return_focus(self.detail_menu.connect_focus.clone());
                });
            }
        } else {
            let tenant = self.plugins.snapshot["tenantId"].clone();
            self.request(
                methods::PLUGINS,
                json!({
                    "action": "connect",
                    "tenantId": tenant,
                    "pluginId": definition.id,
                    "label": label,
                    "requestId": uuid::Uuid::new_v4().to_string(),
                    "returnTo": {"kind": "desktop", "port": 0},
                }),
                cx,
            );
        }
    }

    fn add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_menu_open = false;
        if self.signed_in() {
            self.open_setup(SetupTarget::Server, window, cx);
        } else {
            self.sign_in(cx);
        }
    }

    fn set_enabled(&mut self, connection: Value, enabled: bool, cx: &mut Context<Self>) {
        let tenant = self.plugins.snapshot["tenantId"].clone();
        self.request(
            methods::PLUGINS,
            json!({"action":"setEnabled","tenantId":tenant,"connectionId":connection,"enabled":enabled}),
            cx,
        );
    }

    fn connections(&self) -> Vec<Connection> {
        let catalog = zuse_viewport::plugin_catalog::catalog(&self.plugins.snapshot, "", false);
        self.plugins.snapshot["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| {
                let plugin_id = row["pluginId"].as_str().unwrap_or_default();
                let definition = catalog.iter().find(|item| item.id == plugin_id);
                Connection {
                    id: row["id"].clone(),
                    label: row["label"]
                        .as_str()
                        .filter(|label| !label.is_empty())
                        .map(str::to_owned)
                        .or_else(|| definition.map(|item| item.name.clone()))
                        .unwrap_or_else(|| "Plugin".into()),
                    description: definition
                        .map(|item| item.description.clone())
                        .unwrap_or_default(),
                    domain: definition
                        .map(|item| item.domain.clone())
                        .unwrap_or_default(),
                    state: row["state"].as_str().unwrap_or_default().to_owned(),
                    enabled: row["enabled"] != false,
                }
            })
            .collect()
    }

    fn connected_count(&self) -> usize {
        self.connections()
            .iter()
            .filter(|connection| connection.state == "connected")
            .count()
    }

    /// Progress, notices and prompts both views show above their lists: the
    /// account's device login, errors, the setup form, a removal to confirm
    /// and a hosted sign-in in flight.
    fn render_status(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut items: Vec<AnyElement> = Vec::new();
        let button = |id: &'static str, label: &str| {
            widgets::text_action(theme, widgets::ActionTone::Outlined, label.to_owned())
                .id(id)
                .tab_index(0)
                .role(gpui::Role::Button)
        };
        if let Some(login) = self.account.login.clone() {
            items.push(
                widgets::section_card(theme)
                    .mt_0()
                    .child(
                        widgets::card_row(theme, true)
                            .child(catalog_ui::title_block(
                                theme,
                                "Finish signing in",
                                format!("Confirm the code {} in your browser.", login.user_code),
                                crate::typography::TEXT_SM,
                                crate::typography::TEXT_XS,
                            ))
                            .child(button("open-cloud-login", "Continue in browser").on_click(
                                move |_, _, cx| {
                                    if login.url.starts_with("https://") {
                                        cx.open_url(&login.url);
                                    }
                                },
                            )),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = self.error.clone().or_else(|| self.account.error.clone()) {
            items.push(widgets::error_strip(theme, error).mt_0().into_any_element());
        }
        if let Some(notice) = self.plugins.notice.clone() {
            items.push(
                div()
                    .id("plugin-notice")
                    .role(gpui::Role::Status)
                    .px(px(8.))
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child(notice)
                    .into_any_element(),
            );
        }
        if self.signed_in() && self.plugins.snapshot["canManage"] == false {
            items.push(
                div()
                    .px(px(8.))
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child("Only workspace administrators can manage these connections.")
                    .into_any_element(),
            );
        }
        if let Some(form) = self.setup.clone().filter(|form| !form.read(cx).is_inline()) {
            items.push(div().absolute().child(form).into_any_element());
        }
        if self.plugins.confirmation.is_some() {
            if let Some(dialog) = &self.detail_menu.removal {
                dialog.update(cx, |dialog, cx| {
                    dialog.sync(
                        self.busy,
                        self.error.clone().or(self.plugins.notice.clone()),
                        cx,
                    )
                });
                items.push(div().absolute().child(dialog.clone()).into_any_element());
            }
        }
        if self.plugins.active_attempt() {
            let id = self.plugins.attempt["pluginId"]
                .as_str()
                .unwrap_or_default();
            let name = zuse_viewport::plugin_catalog::catalog(&self.plugins.snapshot, "", false)
                .into_iter()
                .find(|p| p.id == id)
                .map(|p| p.name)
                .unwrap_or_else(|| "plugin".into());
            items.push(zuse_viewport::plugin_detail::pending(&name,self.plugins.attempt["authorizationUrl"].as_str(),self.busy,
                SetupColors{text:theme.text,muted:theme.text_muted,background:theme.surface_card,border:theme.border,danger:theme.danger},cx,
                |this,_,cx|this.request(methods::PLUGINS,json!({"action":"cancel","tenantId":this.plugins.snapshot["tenantId"],"attemptId":this.plugins.attempt["id"]}),cx)));
        }

        items
    }

    /// The signed-out explanation shared by both views.
    fn render_sign_in_card(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        widgets::section_card(theme).mt_0().child(
            widgets::card_row(theme, true)
                .gap(px(12.))
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded(px(10.))
                        .bg(theme.settings_frame_bg())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            crate::icons::icon(crate::icons::SETTINGS_PLUGINS)
                                .size(px(16.))
                                .text_color(theme.text_muted),
                        ),
                )
                .child(catalog_ui::title_block(
                    theme,
                    "Sign in to connect plugins",
                    "Hosted plugins run through your Zuse account. Local agents keep working without one.",
                    crate::typography::TEXT_SM,
                    crate::typography::TEXT_XS,
                ))
                .child(
                    catalog_ui::primary_button(theme, "cloud-account-sign-in")
                        .when(self.busy, |el| el.opacity(0.6))
                        .child("Sign in")
                        .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                ),
        )
    }

    /// The title opens plugin details; the trailing action connects an account.
    fn render_browse_row(
        &self,
        theme: &Theme,
        definition: zuse_protocol::plugins::PluginDefinition,
        section: &'static str,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let connected =
            zuse_viewport::plugin_catalog::connected(&self.plugins.snapshot, &definition.id);
        let selected = definition.id.clone();
        div()
            .id((section, index))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .p(px(8.))
            .rounded(px(10.))
            .hover(|view| view.bg(theme.settings_frame_bg().opacity(0.45)))
            .child(
                div()
                    .id((SharedString::from(format!("{section}-details")), index))
                    .role(gpui::Role::Button)
                    .tab_index(0)
                    .aria_label(format!("Open {} details", definition.name))
                    .cursor_pointer()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(catalog_ui::plugin_logo(
                        theme,
                        &definition.domain,
                        &definition.name,
                        36.,
                    ))
                    .child(catalog_ui::title_block(
                        theme,
                        definition.name.clone(),
                        definition.description.clone(),
                        13.,
                        12.,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.browse_selected = Some(selected.clone());
                        this.detail_menu = Default::default();
                        cx.notify();
                    })),
            )
            .child(
                catalog_ui::icon_button(
                    theme,
                    (SharedString::from(format!("{section}-connect")), index),
                    if connected {
                        crate::icons::CHECK
                    } else {
                        crate::icons::PLUS
                    },
                    if connected {
                        "Connected"
                    } else {
                        "Connect plugin"
                    },
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !connected {
                        this.connect(definition.clone(), window, cx);
                    }
                })),
            )
            .into_any_element()
    }

    fn detail_action(
        &mut self,
        action: zuse_viewport::plugin_detail::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zuse_viewport::plugin_detail::Action;
        let selected = self.browse_selected.as_ref().and_then(|id| {
            zuse_viewport::plugin_catalog::catalog(&self.plugins.snapshot, "", false)
                .into_iter()
                .find(|p| &p.id == id)
        });
        match action {
            Action::Back => {
                self.setup = None;
                self.setup_events = None;
                self.browse_selected = None;
                self.detail_menu = Default::default();
            }
            Action::MenuToggle => self.detail_menu.toggle(),
            Action::MenuClose => self.detail_menu.close(),
            Action::AccountMenu(id) => self.detail_menu.toggle_account(id,window,cx),
            Action::AccountClose => self.detail_menu.close_account(true,window,cx),
            Action::AccountDismiss => self.detail_menu.close_account(false,window,cx),
            Action::Connect => {
                if let Some(definition) = selected {
                    self.connect(definition, window, cx);
                }
            }
            Action::Toggle { id, enabled } => self.set_enabled(json!(id), enabled, cx),
            Action::Disconnect(id) => {
                self.request(methods::PLUGINS,json!({"action":"disconnect","tenantId":self.plugins.snapshot["tenantId"],"connectionId":id}),cx)
            }
            Action::RemoveServer => {
                if let Some(definition) = selected {
                    self.ask_removal(
                        zuse_protocol::plugin_state::Removal::Server,
                        &definition.id,
                        cx,
                    );
                    if self.plugins.confirmation.is_some() {
                        let theme=Theme::of(cx).for_settings_surface();
                        let colors=SetupColors{text:theme.text,muted:theme.text_muted,background:zuse_viewport::palette::rgb(0x191c1a).into(),border:theme.border,danger:theme.danger};
                        let dialog=cx.new(|cx|zuse_viewport::plugin_removal::ServerRemoval::new(definition.name,colors,window,cx));
                        self.detail_menu.removal_events=Some(cx.subscribe(&dialog,|this,_,event,cx| {
                            match event {zuse_viewport::plugin_removal::Event::Cancel=>this.plugins.confirmation=None,zuse_viewport::plugin_removal::Event::Remove=>this.confirm_removal(cx)}cx.notify();
                        }));
                        self.detail_menu.removal=Some(dialog);
                    }
                }
            }
        }
        cx.notify();
    }

    fn render_browse_section(
        &self,
        theme: &Theme,
        title: &'static str,
        section: &'static str,
        items: Vec<zuse_protocol::plugins::PluginDefinition>,
        two_columns: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows = items
            .into_iter()
            .enumerate()
            .map(|(index, item)| self.render_browse_row(theme, item, section, index, cx))
            .collect::<Vec<_>>()
            .into_iter();
        let mut grid = div().flex().flex_col().gap(px(2.));
        while let Some(left) = rows.next() {
            if !two_columns {
                grid = grid.child(left);
                continue;
            }
            let right = rows
                .next()
                .unwrap_or_else(|| div().flex_1().min_w_0().into_any_element());
            grid = grid.child(div().flex().gap(px(16.)).child(left).child(right));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .px(px(8.))
                    .text_size(crate::typography::text_sm())
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_strong)
                    .child(title),
            )
            .child(grid)
            .into_any_element()
    }

    /// The main-area browse page: title, toolbar, Browse / Connected tabs,
    /// then Featured and All plugins as a two-column grid.
    pub fn render_browse(
        &mut self,
        scroll: &gpui::ScrollHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use zuse_viewport::plugin_catalog as catalog;
        let theme = Theme::of(cx).clone();
        zuse_viewport::palette::set_light(theme.appearance.is_light());
        if let Some(definition) = self.browse_selected.as_ref().and_then(|id| {
            catalog::catalog(&self.plugins.snapshot, "", false)
                .into_iter()
                .find(|p| &p.id == id)
        }) {
            let colors = SetupColors {
                text: theme.text,
                muted: theme.text_muted,
                background: theme.surface_card,
                border: theme.border,
                danger: theme.danger,
            };
            let status = self.render_status(&theme, cx);
            return div()
                .id("plugins-page")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(
                    div()
                        .w_full()
                        .max_w(px(768.))
                        .mx_auto()
                        .px(px(32.))
                        .py(px(32.))
                        .flex()
                        .flex_col()
                        .gap(px(20.))
                        .children(status)
                        .child(zuse_viewport::plugin_detail::render(
                            &definition,
                            &self.plugins.snapshot,
                            self.busy || self.plugins.active_attempt(),
                            &mut self.detail_menu,
                            self.setup.clone().filter(|form| form.read(cx).is_inline()),
                            colors,
                            cx,
                            Self::detail_action,
                        )),
                )
                .into_any_element();
        }
        let query = self.search.read(cx).text().trim().to_owned();
        let items = catalog::catalog(&self.plugins.snapshot, &query, self.connected_only);
        let connected = self.connected_count();
        let mut sections: Vec<AnyElement> = Vec::new();
        if items.is_empty() {
            sections.push(
                div()
                    .px(px(8.))
                    .py(px(24.))
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child(if self.connected_only && query.is_empty() {
                        "No connected plugins yet. Connect one from Browse."
                    } else {
                        "No plugins match your search."
                    })
                    .into_any_element(),
            );
        } else if self.connected_only {
            sections.push(self.render_browse_section(
                &theme,
                "Connected",
                "plugin-connected",
                items,
                window.viewport_size().width >= px(1024.),
                cx,
            ));
        } else {
            if query.is_empty() {
                let featured: Vec<_> = items
                    .iter()
                    .filter(|item| item.featured)
                    .take(FEATURED_LIMIT)
                    .cloned()
                    .collect();
                if !featured.is_empty() {
                    sections.push(self.render_browse_section(
                        &theme,
                        "Featured",
                        "plugin-featured",
                        featured,
                        window.viewport_size().width >= px(1024.),
                        cx,
                    ));
                }
            }
            let mut all = items;
            all.sort_by_key(|item| item.name.to_lowercase());
            sections.push(self.render_browse_section(
                &theme,
                "All plugins",
                "plugin-all",
                all,
                window.viewport_size().width >= px(1024.),
                cx,
            ));
        }
        let tabs = catalog_ui::segmented_tabs(
            &theme,
            "plugins-tab",
            &[("Browse", None), ("Connected", Some(connected))],
            usize::from(self.connected_only),
            |index, tab| {
                tab.on_click(cx.listener(move |this, _, _, cx| {
                    this.connected_only = index == 1;
                    cx.notify();
                }))
            },
        );
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(catalog_ui::search_field(
                &theme,
                Some(224.),
                self.search.clone().into_any_element(),
            ))
            .child(
                catalog_ui::secondary_button(&theme, "add-hosted-server")
                    .child("Add MCP server")
                    .on_click(cx.listener(|this, _, window, cx| this.add_server(window, cx))),
            )
            .child(
                catalog_ui::icon_button(
                    &theme,
                    "refresh-hosted-plugins",
                    crate::icons::REFRESH,
                    "Refresh plugins",
                )
                .when(self.busy, |el| el.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        let status = self.render_status(&theme, cx);
        let sign_in = (!self.signed_in()).then(|| self.render_sign_in_card(&theme, cx));
        div()
            .id("plugins-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(scroll)
            .text_color(theme.text)
            .child(
                div()
                    .w_full()
                    .max_w(px(768.))
                    .mx_auto()
                    .px(px(32.))
                    .pt(px(32.))
                    .pb(px(32.))
                    .flex()
                    .flex_col()
                    .gap(px(20.))
                    .text_size(crate::typography::text_xs())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(20.))
                                    .line_height(crate::typography::ui_rems(28.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("Plugins"),
                            )
                            .child(
                                div()
                                    .text_size(crate::typography::text_sm())
                                    .text_color(theme.text_muted)
                                    .child("Connect your tools so agents can work across them in any chat."),
                            ),
                    )
                    .child(toolbar)
                    .child(div().flex().child(tabs))
                    .children(sign_in)
                    .children(status)
                    .children(sections),
            )
            .into_any_element()
    }

    /// Settings → Plugins when signed in: tabs, search and Add on one row,
    /// then the Connected frame.
    fn render_installed(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let query = self.installed_search.read(cx).text().trim().to_lowercase();
        let connections = self.connections();
        let total = connections.len();
        let shown: Vec<Connection> = connections
            .into_iter()
            .filter(|connection| {
                query.is_empty()
                    || format!("{} {}", connection.label, connection.description)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        let tabs = catalog_ui::segmented_tabs(
            theme,
            "plugins-settings-tab",
            &[("Plugins", Some(total))],
            0,
            |_, tab| tab,
        );
        let popup = theme.for_popup();
        let add_menu = self.add_menu_open.then(|| {
            let row = |id: &'static str, icon: &'static str, label: &'static str| {
                popover::menu_row(&popup, false, id)
                    .id(id)
                    .role(gpui::Role::MenuItem)
                    .child(
                        crate::icons::icon(icon)
                            .size(px(14.))
                            .text_color(popup.text_muted),
                    )
                    .child(label)
            };
            popover::anchored_menu_below_end(
                "plugins-add-menu",
                popover::popover_card(&popup)
                    .w(px(200.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.add_menu_open = false;
                        this.add_menu_closed = Some(std::time::Instant::now());
                        cx.notify();
                    }))
                    .child(
                        row(
                            "plugins-add-browse",
                            crate::icons::SETTINGS_PLUGINS,
                            "Browse plugins",
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_menu_open = false;
                            window.dispatch_action(Box::new(OpenPluginsPage), cx);
                        })),
                    )
                    .child(
                        row("plugins-add-server", crate::icons::PLUS, "Add MCP server").on_click(
                            cx.listener(|this, _, window, cx| this.add_server(window, cx)),
                        ),
                    )
                    .into_any_element(),
                None,
            )
        });
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(tabs)
            .child(div().flex_1())
            .child(catalog_ui::search_field(
                theme,
                Some(192.),
                self.installed_search.clone().into_any_element(),
            ))
            .child(
                div()
                    .relative()
                    .child(
                        catalog_ui::secondary_button(theme, "plugins-add")
                            .aria_expanded(self.add_menu_open)
                            .child("Add")
                            .child(
                                crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                                    .size(px(12.))
                                    .text_color(theme.text_muted),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let just_closed = this.add_menu_closed.is_some_and(|at| {
                                    at.elapsed() < std::time::Duration::from_millis(300)
                                });
                                this.add_menu_open = !this.add_menu_open && !just_closed;
                                this.add_menu_closed = None;
                                cx.notify();
                            })),
                    )
                    .children(add_menu),
            );
        let body: AnyElement = if shown.is_empty() {
            div()
                .py(px(28.))
                .px(px(16.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.))
                .child(widgets::row_title(
                    theme,
                    if total == 0 {
                        "No plugins connected yet"
                    } else {
                        "No connected plugins match your search"
                    },
                ))
                .when(total == 0, |el| {
                    el.child(
                        div()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child("Connect tools from the catalog so agents can use them in any chat."),
                    )
                    .child(
                        catalog_ui::secondary_button(theme, "plugins-browse")
                            .mt(px(10.))
                            .child("Browse plugins")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenPluginsPage), cx)
                            }),
                    )
                })
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(shown.into_iter().enumerate().map(|(index, connection)| {
                    let id = connection.id.clone();
                    let remove_id = connection.id.as_str().unwrap_or_default().to_owned();
                    let enabled = connection.enabled;
                    let group = SharedString::from(format!("plugin-connection-{index}"));
                    let description = match connection.state.as_str() {
                        "connected" | "" => connection.description.clone(),
                        "needs-auth" => "Needs authorization".into(),
                        "connecting" => "Connecting…".into(),
                        "error" => "Connection failed — reconnect from the catalog".into(),
                        other => other.to_owned(),
                    };
                    widgets::card_row(theme, index == 0)
                        .id(("plugin-connection", index))
                        .group(group.clone())
                        .flex_nowrap()
                        .gap(px(12.))
                        .child(catalog_ui::plugin_logo(
                            theme,
                            &connection.domain,
                            &connection.label,
                            32.,
                        ))
                        .child(catalog_ui::title_block(
                            theme,
                            connection.label.clone(),
                            description,
                            crate::typography::TEXT_SM,
                            crate::typography::TEXT_XS,
                        ))
                        .child(
                            widgets::ghost_action(theme)
                                .id(("disconnect-plugin", index))
                                .opacity(0.)
                                .group_hover(group, |s| s.opacity(1.))
                                .tab_index(0)
                                .role(gpui::Role::Button)
                                .child("Disconnect")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.request(methods::PLUGINS,json!({"action":"disconnect","tenantId":this.plugins.snapshot["tenantId"],"connectionId":remove_id}),cx)
                                })),
                        )
                        .child(
                            widgets::toggle_switch(
                                theme,
                                enabled,
                                format!("plugin-switch-{index}"),
                            )
                            .id(("toggle-plugin", index))
                            .cursor_pointer()
                            .tab_index(0)
                            .role(gpui::Role::Switch)
                            .aria_label(connection.label.clone())
                            .aria_toggled(if enabled {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.set_enabled(id.clone(), !enabled, cx),
                            )),
                        )
                        .into_any_element()
                }))
                .into_any_element()
        };
        let tenants: Vec<Value> = self.plugins.snapshot["tenants"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        // Organization workspaces are hidden for now (they return later as
        // their own feature); the engine's current tenant stays in effect.
        let workspaces = (SHOW_PLUGIN_WORKSPACES && tenants.len() > 1).then(|| {
            let current = self.plugins.snapshot["tenantId"].clone();
            widgets::group(
                theme,
                "Workspace",
                "Applies when a chat first uses plugins. Existing plugin sessions keep their workspace.",
            )
            .child(widgets::section_card(theme).mt_0().children(tenants.into_iter().enumerate().map(
                |(index, tenant)| {
                    let selected = tenant["id"] == current;
                    let id = tenant["id"].clone();
                    widgets::card_row(theme, index == 0)
                        .id(("plugin-tenant", index))
                        .cursor_pointer()
                        .child(
                            div()
                                .flex_1()
                                .child(widgets::row_title(
                                    theme,
                                    tenant["name"].as_str().unwrap_or("Workspace").to_owned(),
                                )),
                        )
                        .when(selected, |row| {
                            row.child(crate::icons::icon(crate::icons::CHECK).size(px(14.)).text_color(theme.text_muted))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request(
                                methods::SELECT_PLUGIN_WORKSPACE,
                                json!({"action":"list","tenantId":id}),
                                cx,
                            )
                        }))
                },
            )))
        });
        let status = self.render_status(theme, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .text_color(theme.text)
            .child(header)
            .children(status)
            .child(
                widgets::group(theme, "Connected", "")
                    .child(widgets::section_card(theme).mt_0().child(body)),
            )
            .children(workspaces)
            .into_any_element()
    }

    fn render_account(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use zuse_viewport::account_settings::{AccountCard, Colors, Event, Properties};
        if self.account_view.is_none() {
            let view = cx.new(|cx| AccountCard::new(window, cx));
            self.account_view_events = Some(cx.subscribe(&view, |this, _, event, cx| {
                match event {
                    Event::SignIn => this.request(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}), cx),
                    Event::SignOut => this.request(methods::CLOUD_ACCOUNT_SIGN_OUT, json!({}), cx),
                    Event::DisplayName { account, value } => {
                        if this
                            .account
                            .user
                            .as_ref()
                            .is_none_or(|user| &user.id != account)
                        {
                            return;
                        }
                        crate::settings::update(
                            crate::settings::SavePolicy::Immediate,
                            cx,
                            |settings| settings.display_name = value.clone(),
                        );
                        cx.refresh_windows();
                    }
                }
                cx.notify();
            }));
            self.account_view = Some(view);
        }
        let mut status = serde_json::to_value(&self.account).unwrap_or_default();
        if let Some(error) = &self.error {
            status["error"] = json!(error);
        }
        let properties = Properties {
            status,
            display_name: crate::settings::current(cx).display_name,
            loaded: self.account_loaded || self.error.is_some(),
            available: self.state.read(cx).engine().is_some(),
            busy: self.busy,
            colors: Colors {
                text: theme.text,
                muted: theme.text_muted,
                background: theme.surface_card,
                border: theme.border,
                danger: zuse_viewport::settings_style::destructive_text(),
            },
        };
        let view = self.account_view.as_ref().unwrap();
        view.update(cx, |view, cx| view.set_properties(properties, cx));
        view.clone().into_any_element()
    }
}

impl Render for PluginsCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        if self.account_only {
            return self.render_account(&theme, window, cx);
        }
        if !self.signed_in() {
            let status = self.render_status(&theme, cx);
            return div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .text_color(theme.text)
                .child(
                    widgets::group(
                        &theme,
                        "Hosted plugins",
                        "Plugins give agents access to the tools you already use, in any chat.",
                    )
                    .child(self.render_sign_in_card(&theme, cx)),
                )
                .children(status)
                .into_any_element();
        }
        self.render_installed(&theme, cx)
    }
}

/// The main-area Plugins page: the shared [`PluginsCard`] as a catalog.
pub struct PluginsPage {
    card: Entity<PluginsCard>,
    scroll: gpui::ScrollHandle,
    _card: Subscription,
}

struct SharedPluginsPage(Entity<PluginsPage>);
impl Global for SharedPluginsPage {}

impl PluginsPage {
    pub fn shared(state: Entity<AppState>, cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedPluginsPage>() {
            return shared.0.clone();
        }
        let card = PluginsCard::shared(state, cx);
        let page = cx.new(|cx| Self {
            _card: cx.observe(&card, |_, _, cx| cx.notify()),
            card,
            scroll: gpui::ScrollHandle::new(),
        });
        cx.set_global(SharedPluginsPage(page.clone()));
        page
    }
}

impl Render for PluginsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scroll = self.scroll.clone();
        self.card
            .update(cx, |card, cx| card.render_browse(&scroll, window, cx))
    }
}

/// Review fixtures: `signedout`, `empty`, or (default) three connections.
#[cfg(feature = "browser-fixture")]
pub fn fixture_seed(state: Entity<AppState>, variant: &str, cx: &mut App) {
    let card = PluginsCard::shared(state, cx);
    let variant = variant.to_owned();
    card.update(cx, |card, cx| {
        card.fixture = true;
        card.watch = None;
        card.account = if variant == "signedout" {
            AccountStatus::default()
        } else {
            serde_json::from_value(
                json!({"user":{"id":"fixture","email":"alex@example.test","name":"Alex"}}),
            )
            .unwrap_or_default()
        };
        let connections = if variant == "empty" || variant == "signedout" {
            json!([])
        } else {
            json!([
                {"id":"c-linear","pluginId":"linear","label":"Linear","state":"connected","enabled":true},
                {"id":"c-sentry","pluginId":"sentry","label":"Sentry","state":"connected","enabled":true},
                {"id":"c-notion","pluginId":"notion","label":"Notion","state":"connected","enabled":false},
                {"id":"c-figma","pluginId":"figma","label":"Figma","state":"needs-auth","enabled":true},
            ])
        };
        card.plugins.snapshot =
            json!({"tenantId":"fixture","canManage":true,"connections":connections,
                "tenants":[{"id":"fixture","name":"Personal"},{"id":"org","name":"Fixture org"}]});
        cx.notify();
    });
}
