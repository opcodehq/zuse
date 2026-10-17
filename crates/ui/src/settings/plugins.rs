//! Account and hosted connections, backed by the engine's shared authority.
use crate::{settings::widgets, state::AppState, theme::Theme};
use gpui::{Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};
use zuse_engine::account::AccountStatus;
use zuse_rpc::methods;

pub struct PluginsCard {
    state: Entity<AppState>,
    account: AccountStatus,
    snapshot: Value,
    attempt: Value,
    error: Option<String>,
    epoch: u64,
    busy: bool,
    watch: Option<Task<()>>,
    action: Option<Task<()>>,
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
                this.account = AccountStatus::default();
                this.snapshot = Value::Null;
                this.attempt = Value::Null;
            } else if this.watch.is_none() {
                this.attach(cx);
            }
        });
        let mut card = Self {
            state,
            account: AccountStatus::default(),
            snapshot: Value::Null,
            attempt: Value::Null,
            error: None,
            epoch: 0,
            busy: false,
            watch: None,
            action: None,
            _state: subscription,
        };
        card.attach(cx);
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
                                    card.epoch = card.epoch.wrapping_add(1);
                                    card.snapshot = Value::Null;
                                    card.attempt = Value::Null;
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
    fn request(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
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
                    Ok(value) if method == methods::PLUGINS => match value["kind"].as_str() {
                        Some("snapshot") => card.snapshot = value,
                        Some("attempt") => {
                            let complete = value["state"] == "connected";
                            card.attempt = value;
                            if complete {
                                card.request(methods::PLUGINS, json!({"action":"list"}), cx);
                            }
                        }
                        _ => {
                            card.attempt = Value::Null;
                            card.request(methods::PLUGINS, json!({"action":"list"}), cx);
                        }
                    },
                    Ok(_) => {}
                    Err(error) => card.error = Some(error.to_string()),
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
        let tenant = self.snapshot["tenantId"]
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
                .child(action("refresh-hosted-plugins",if self.busy {"Loading…"} else {"Refresh plugins"}).on_click(cx.listener(|this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"list"}),cx))))
                .children(self.snapshot["tenants"].as_array().into_iter().flatten().enumerate().map(|(i,t)|{
                    let id=t["id"].clone();
                    widgets::ghost_action(&theme).id(("plugin-tenant",i)).child(SharedString::from(t["name"].as_str().unwrap_or("Workspace").to_owned())).on_click(cx.listener(move |this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"list","tenantId":id}),cx)))
                }))
                .children(self.snapshot["connections"].as_array().into_iter().flatten().enumerate().map(|(i,c)|{
                    let id=c["id"].clone();let tenant=tenant.clone();let enabled=c["enabled"]!=false;
                    div().flex().items_center().gap(px(10.)).child(div().flex_1().child(SharedString::from(c["label"].as_str().unwrap_or("Plugin").to_owned())))
                        .child(SharedString::from(c["state"].as_str().unwrap_or_default().to_owned()))
                        .child(widgets::ghost_action(&theme).id(("toggle-plugin",i)).child(if enabled {"Disable"} else {"Enable"}).on_click(cx.listener(move |this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"setEnabled","tenantId":tenant,"connectionId":id,"enabled":!enabled}),cx))))
                }))
                .children(self.snapshot["catalog"].as_array().into_iter().flatten().enumerate().map(|(i,p)|{
                    let id=p["id"].clone();let name=p["name"].as_str().unwrap_or("Plugin").to_owned();let tenant=tenant.clone();
                    div().flex().items_center().gap(px(12.)).child(div().flex_1().child(name.clone()))
                        .child(widgets::ghost_action(&theme).id(("connect-plugin",i)).child("Connect").on_click(cx.listener(move |this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"connect","tenantId":tenant,"pluginId":id,"label":name,"requestId":uuid::Uuid::new_v4().to_string(),"returnTo":{"kind":"desktop","port":0}}),cx))))
                })))
            .when_some(self.attempt["authorizationUrl"].as_str().filter(|s|s.starts_with("https://")).map(str::to_owned),|d,url|d.child(action("authorize-hosted-plugin","Authorize connection").on_click(move |_,_,cx|cx.open_url(&url))))
            .when(self.attempt["id"].is_string(),|d|{
                let id=self.attempt["id"].clone();let tenant=tenant.clone();
                d.child(action("poll-hosted-plugin","Check connection").on_click(cx.listener(move |this,_,_,cx|this.request(methods::PLUGINS,json!({"action":"poll","tenantId":tenant,"attemptId":id}),cx))))
            })
    }
}
