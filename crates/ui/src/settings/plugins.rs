//! Account and hosted connections, backed by the engine's shared authority.
use crate::{settings::widgets, state::AppState, theme::Theme};
use gpui::{Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};
use zuse_engine::account::AccountStatus;
use zuse_rpc::methods;
use zuse_viewport::plugin_setup::{PluginSetup, SetupColors, SetupEvent, SetupTarget};

pub struct PluginsCard {
    state: Entity<AppState>,
    account_only: bool,
    account: AccountStatus,
    plugins: zuse_protocol::plugin_state::PluginState,
    error: Option<String>,
    epoch: u64,
    busy: bool,
    watch: Option<Task<()>>,
    action: Option<Task<()>>,
    setup: Option<Entity<PluginSetup>>,
    setup_events: Option<Subscription>,
    _state: Subscription,
}
impl PluginsCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.state.read(cx).engine().is_none() {
                this.watch = None;
                this.action = None;
                this.busy = false;
                this.epoch = this.epoch.wrapping_add(1);
                this.setup = None;
                this.setup_events = None;
                this.account = AccountStatus::default();
                this.plugins = Default::default();
            } else if this.watch.is_none() {
                this.attach(cx);
            }
        });
        let mut card = Self {
            state,
            account_only: false,
            account: AccountStatus::default(),
            plugins: Default::default(),
            error: None,
            epoch: 0,
            busy: false,
            watch: None,
            action: None,
            setup: None,
            setup_events: None,
            _state: subscription,
        };
        card.attach(cx);
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
                                if changed {
                                    card.setup = None;
                                    card.setup_events = None;
                                    card.epoch = card.epoch.wrapping_add(1);
                                    card.plugins = Default::default();
                                    card.action = None;
                                    card.busy = false;
                                    if card.account.user.is_some() {
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
            background: theme.surface_card,
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
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Render for PluginsCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let signed_in = self.account.user.is_some();
        if self.account_only {
            let title = self
                .account
                .user
                .as_ref()
                .map(|user| user.name.clone().unwrap_or_else(|| user.email.clone()))
                .unwrap_or_else(|| "Not signed in".into());
            let description = self.account.user.as_ref().map(|user|user.email.clone()).unwrap_or_else(||"You're using Zuse (Beta) locally without an account. Sign in to sync and unlock remote agents.".into());
            let account = widgets::card_row(&theme, true)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(&theme, title))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.))
                                .line_height(crate::typography::ui_rems(16.))
                                .text_color(theme.text_muted)
                                .child(description),
                        ),
                )
                .child(
                    widgets::text_action(
                        &theme,
                        widgets::ActionTone::Outlined,
                        if signed_in { "Sign out" } else { "Sign in" },
                    )
                    .id("general-account-sign-in")
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request(
                            if signed_in {
                                methods::CLOUD_ACCOUNT_SIGN_OUT
                            } else {
                                methods::CLOUD_ACCOUNT_SIGN_IN
                            },
                            json!({}),
                            cx,
                        )
                    })),
                );
            return widgets::group(&theme,"Account","Sign in to sync your account across devices and (soon) drive remote agents from your phone.")
                .child(widgets::section_card(&theme).mt_0().child(account))
                .when_some(self.account.login.clone(),|view,login|view.child(widgets::text_action(&theme,widgets::ActionTone::Outlined,"Open sign-in").id("general-open-login")
                    .on_click(move|_,_,cx|{if login.url.starts_with("https://"){cx.open_url(&login.url);}})))
                .when_some(self.error.clone().or_else(||self.account.error.clone()),|view,error|view.child(widgets::error_strip(&theme,error)))
                .into_any_element();
        }
        let tenant = self.plugins.snapshot["tenantId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let action = |id: &'static str, text: &str| {
            widgets::ghost_action(&theme)
                .id(id)
                .tab_index(0)
                .role(gpui::Role::Button)
                .child(SharedString::from(text.to_owned()))
        };
        div().flex().flex_col().gap(px(12.)).py(px(16.)).text_color(theme.text)
            .child(div().flex().items_center().gap(px(12.))
                .child(div().flex_1().child("Zuse account & plugins"))
                .child(action("cloud-account-sign-in",if signed_in {"Sign out"} else {"Sign in"}).on_click(cx.listener(move |this,_,_,cx|this.request(if signed_in {methods::CLOUD_ACCOUNT_SIGN_OUT} else {methods::CLOUD_ACCOUNT_SIGN_IN},json!({}),cx)))))
            .child(div().text_size(px(12.)).text_color(theme.text_muted).child(self.account.user.as_ref().map(|u|u.email.clone()).unwrap_or_else(||"Local agents work without a Zuse account. Sign in for hosted plugins.".into())))
            .when_some(self.account.login.clone(),|d,login|d.child(div().flex().items_center().gap(px(12.)).child(format!("Authorization code: {}",login.user_code)).child(action("open-cloud-login","Open sign-in").on_click(move |_,_,cx|{if login.url.starts_with("https://"){cx.open_url(&login.url);}}))))
            .when_some(self.error.clone().or_else(||self.account.error.clone()),|d,error|d.child(div().text_size(px(12.)).text_color(theme.danger).child(error)))
            .when(signed_in,|d|d
                .child(div().text_size(px(12.)).text_color(theme.text_muted).child("Workspace selection applies when a chat first uses plugins. Existing plugin sessions keep their workspace."))
                .child(action("add-hosted-server","Add MCP server").on_click(cx.listener(|this,_,window,cx|this.open_setup(SetupTarget::Server,window,cx))))
                .when(self.plugins.snapshot["canManage"]==false,|d|d.child(widgets::row_title(&theme,"Only workspace administrators can manage these connections.")))
                .when_some(self.setup.clone(),|d,form|d.child(form))
                .when_some(self.plugins.notice.clone(),|d,notice|d.child(div().id("plugin-notice").role(gpui::Role::Status).child(notice)))
                .when_some(self.plugins.confirmation.as_ref(),|d,confirmation|d.child(div().flex().flex_col().gap(px(8.)).p(px(12.)).border_1().rounded(px(8.)).border_color(theme.border)
                    .child(confirmation.title()).child(confirmation.label.clone())
                    .child(div().flex().gap(px(8.)).child(action("dismiss-plugin-removal","Keep it").on_click(cx.listener(|this,_,_,cx|{this.plugins.confirmation=None;cx.notify();})))
                        .child(action("confirm-plugin-removal",confirmation.button()).on_click(cx.listener(|this,_,_,cx|this.confirm_removal(cx)))))))
                .child(action("refresh-hosted-plugins",if self.busy {"Loading…"} else {"Refresh plugins"}).on_click(cx.listener(|this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"list"}),cx))))
                .children(self.plugins.snapshot["tenants"].as_array().into_iter().flatten().enumerate().map(|(i,t)|{
                    let id=t["id"].clone();
                    widgets::ghost_action(&theme).id(("plugin-tenant",i)).when(t["id"]==self.plugins.snapshot["tenantId"],|d|d.text_color(theme.accent)).child(SharedString::from(t["name"].as_str().unwrap_or("Workspace").to_owned())).on_click(cx.listener(move |this,_,_,cx|this.request(methods::SELECT_PLUGIN_WORKSPACE,json!({"action":"list","tenantId":id}),cx)))
                }))
                .children(self.plugins.snapshot["connections"].as_array().into_iter().flatten().enumerate().map(|(i,c)|{
                    let id=c["id"].clone();let tenant=tenant.clone();let enabled=c["enabled"]!=false;let remove_id=id.as_str().unwrap_or_default().to_owned();
                    div().flex().items_center().gap(px(10.)).child(div().flex_1().child(SharedString::from(c["label"].as_str().unwrap_or("Plugin").to_owned())))
                        .child(SharedString::from(c["state"].as_str().unwrap_or_default().to_owned()))
                        .child(widgets::ghost_action(&theme).id(("toggle-plugin",i)).child(if enabled {"Disable"} else {"Enable"}).on_click(cx.listener(move |this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"setEnabled","tenantId":tenant,"connectionId":id,"enabled":!enabled}),cx))))
                        .child(widgets::ghost_action(&theme).id(("disconnect-plugin",i)).tab_index(0).role(gpui::Role::Button).child("Disconnect").on_click(cx.listener(move|this,_,_,cx|this.ask_removal(zuse_protocol::plugin_state::Removal::Connection,&remove_id,cx))))
                }))
                .children(self.plugins.snapshot["catalog"].as_array().into_iter().flatten().enumerate().map(|(i,p)|{
                    let id=p["id"].clone();let name=p["name"].as_str().unwrap_or("Plugin").to_owned();let tenant=tenant.clone();let remove_id=id.as_str().unwrap_or_default().to_owned();let custom=p["custom"]==true;
                    div().flex().items_center().gap(px(12.)).child(div().flex_1().child(name.clone()))
                        .child(widgets::ghost_action(&theme).id(("connect-plugin",i)).child("Connect").on_click(cx.listener(move |this,_,window,cx|{
                            let definition=this.plugins.snapshot["catalog"].as_array().into_iter().flatten().find(|p|p["id"]==id).cloned().and_then(|p|serde_json::from_value::<zuse_protocol::plugins::PluginDefinition>(p).ok());
                            if let Some(definition)=definition.filter(|p|p.api_key_header.is_some()) {this.open_setup(SetupTarget::Catalog(definition),window,cx);}
                            else {this.request(methods::PLUGINS,json!({"action":"connect","tenantId":tenant,"pluginId":id,"label":name,"requestId":uuid::Uuid::new_v4().to_string(),"returnTo":{"kind":"desktop","port":0}}),cx);}
                        })))
                        .when(custom,|d|d.child(widgets::ghost_action(&theme).id(("remove-plugin-server",i)).tab_index(0).role(gpui::Role::Button).child("Remove").on_click(cx.listener(move|this,_,_,cx|this.ask_removal(zuse_protocol::plugin_state::Removal::Server,&remove_id,cx)))))
                })))
            .when_some(self.plugins.attempt["authorizationUrl"].as_str().filter(|s|s.starts_with("https://")).map(str::to_owned),|d,url|d.child(action("authorize-hosted-plugin","Authorize connection").on_click(move |_,_,cx|cx.open_url(&url))))
            .when(self.plugins.active_attempt(),|d|{
                let id=self.plugins.attempt["id"].clone();let tenant=tenant.clone();
                d.child(action("poll-hosted-plugin","Check connection").on_click(cx.listener(move |this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"poll","tenantId":tenant,"attemptId":id}),cx))))
                    .child(action("cancel-plugin-attempt","Cancel sign-in").on_click(cx.listener(|this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"cancel","tenantId":this.plugins.snapshot["tenantId"],"attemptId":this.plugins.attempt["id"]}),cx))))
            }).into_any_element()
    }
}
