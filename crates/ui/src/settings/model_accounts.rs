//! Direct model connections owned by the engine, separate from CLI logins.
use crate::{composer::ComposerInput, settings::widgets, state::AppState, theme::Theme};
use gpui::{Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px};
use serde_json::{Value, json};
use zuse_engine::account::AccountStatus;
use zuse_rpc::methods;

pub struct ModelAccountsCard {
    state: Entity<AppState>,
    snapshot: Value,
    login: Value,
    storage: &'static str,
    error: Option<String>,
    editing: Option<String>,
    name: Entity<ComposerInput>,
    callback: Entity<ComposerInput>,
    epoch: u64,
    busy: bool,
    loading: bool,
    account_watch: Option<Task<()>>,
    load: Option<Task<()>>,
    action: Option<Task<()>>,
    sign_in: Option<Task<()>>,
    _state: Subscription,
}
impl ModelAccountsCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.state.read(cx).engine().is_none() {
                this.reset();
                this.callback.update(cx, |input, cx| input.set_text("", cx));
                this.account_watch = None;
                cx.notify();
            } else if this.account_watch.is_none() {
                this.attach(cx);
            }
        });
        let name = cx.new(|cx| ComposerInput::new("Connection name", cx));
        let callback = cx.new(|cx| ComposerInput::new("Paste the localhost callback URL", cx));
        let mut card = Self {
            state,
            snapshot: Value::Null,
            login: Value::Null,
            storage: "local",
            error: None,
            editing: None,
            name,
            callback,
            epoch: 0,
            busy: false,
            loading: false,
            account_watch: None,
            load: None,
            action: None,
            sign_in: None,
            _state: subscription,
        };
        card.attach(cx);
        card
    }
    fn reset(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.load = None;
        self.action = None;
        self.sign_in = None;
        self.snapshot = Value::Null;
        self.login = Value::Null;
        self.editing = None;
        self.error = None;
        self.busy = false;
        self.loading = false;
    }
    fn attach(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.refresh(cx);
        self.account_watch = Some(cx.spawn(async move |this, cx| {
            let Ok(mut stream) = engine
                .client()
                .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
            else {
                return;
            };
            let mut subject = None;
            while let Some(value) = stream.recv().await {
                let Ok(account) = serde_json::from_value::<AccountStatus>(value) else {
                    continue;
                };
                let next = account.user.map(|u| u.id);
                if subject == next {
                    continue;
                }
                subject = next;
                if this
                    .update(cx, |card, cx| {
                        card.reset();
                        card.callback.update(cx, |input, cx| input.set_text("", cx));
                        card.storage = "local";
                        card.refresh(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let epoch = self.epoch;
        self.loading = true;
        self.load = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::MODEL_CONNECTIONS, json!({}))
                .await;
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                card.loading = false;
                match result {
                    Ok(value) => card.snapshot = value,
                    Err(error) => card.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
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
                if card.epoch != epoch { return; }
                card.busy = false;
                match result {
                    Ok(value) => {
                        card.editing = None;
                        if method == methods::MODEL_DISCONNECT && value["revoked"] == false {
                            card.error = Some("Disconnected here. Provider revocation could not be confirmed; you can also revoke access in your provider account.".into());
                        }
                        card.refresh(cx);
                    }
                    Err(error) => card.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn connect(&mut self, provider: String, id: Option<String>, cx: &mut Context<Self>) {
        if self.sign_in.is_some() || self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let epoch = self.epoch;
        let storage = self.storage;
        self.error = None;
        self.callback.update(cx, |input, cx| input.set_text("", cx));
        self.login = json!({"event":{"kind":"starting"}});
        self.sign_in = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .subscribe_scoped(
                    methods::MODEL_CONNECT,
                    json!({"provider":provider,"connectionId":id,"storage":storage}),
                )
                .await;
            let mut ended = false;
            match result {
                Ok(mut stream) => {
                    while let Some(frame) = stream.recv().await {
                        let terminal =
                            matches!(frame["event"]["kind"].as_str(), Some("connected" | "error"));
                        if this
                            .update(cx, |card, cx| {
                                if card.epoch != epoch {
                                    return;
                                }
                                if frame["event"]["kind"] == "error" {
                                    card.error = Some(
                                        frame["event"]["message"]
                                            .as_str()
                                            .unwrap_or("Sign-in failed. Try again.")
                                            .into(),
                                    );
                                }
                                card.login = frame;
                                if terminal {
                                    card.refresh(cx);
                                }
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                        if terminal {
                            ended = true;
                            break;
                        }
                    }
                }
                Err(error) => {
                    let _ = this.update(cx, |card, _| {
                        if card.epoch == epoch {
                            card.error = Some(error.to_string());
                        }
                    });
                }
            }
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                card.sign_in = None;
                card.login = Value::Null;
                card.callback.update(cx, |input, cx| input.set_text("", cx));
                if !ended && card.error.is_none() {
                    card.error =
                        Some("Sign-in ended before authorization completed. Try again.".into());
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        let id = self.login["connectionId"].clone();
        self.sign_in = None;
        self.login = Value::Null;
        self.callback.update(cx, |input, cx| input.set_text("", cx));
        if id.is_string() {
            self.request(methods::MODEL_CANCEL_LOGIN, json!({"connectionId":id}), cx);
        }
        cx.notify();
    }
}
impl Render for ModelAccountsCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let action = |id: &'static str, text: &str| {
            widgets::text_action(&theme, widgets::ActionTone::Outlined, text)
                .id(id)
                .tab_index(0)
                .role(gpui::Role::Button)
        };
        let busy = self.busy || self.sign_in.is_some();
        let account_available = self.snapshot["accountAvailable"] == true;
        let local_available = self.snapshot["localAvailable"] == true;
        let connections = self.snapshot["connections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let event = self.login["event"].clone();
        let url = event["url"]
            .as_str()
            .or_else(|| event["verificationUrl"].as_str())
            .filter(|s| s.starts_with("https://"))
            .map(str::to_owned);
        let content = div().flex().flex_col().gap(px(12.)).p(px(12.)).text_size(crate::typography::ui_rems(12.)).text_color(theme.text)
            .child(div().flex().items_center().gap(px(8.))
                .child(action("model-storage-local", if self.storage == "local" { "✓ This device" } else { "This device" }).on_click(cx.listener(|this,_,_,cx|{this.storage="local";cx.notify();})))
                .when(account_available, |d| d.child(action("model-storage-account", if self.storage == "account" { "✓ Zuse account" } else { "Zuse account" }).on_click(cx.listener(|this,_,_,cx|{this.storage="account";cx.notify();}))))
                .when(!busy && ((self.storage == "local" && local_available) || (self.storage == "account" && account_available)), |d|d
                    .child(action("connect-chatgpt", "Connect ChatGPT").on_click(cx.listener(|this,_,_,cx|this.connect("chatgpt".into(),None,cx))))
                    .child(action("connect-supergrok", "Connect SuperGrok").on_click(cx.listener(|this,_,_,cx|this.connect("supergrok".into(),None,cx))))))
            .when(self.snapshot["accountError"] == true, |d|d.child(div().text_size(px(12.)).text_color(theme.warning).child("Account storage is unavailable. Connections on this device remain available.")))
            .when_some(self.error.clone(),|d,error|d.child(div().text_size(px(12.)).text_color(theme.danger).child(error)))
            .when(self.sign_in.is_some(), |d|d.child(div().flex().flex_col().gap(px(8.))
                .child(if event["kind"] == "starting" { "Starting sign-in…" } else { "Finish authorization in your browser." })
                .when_some(event["userCode"].as_str().map(str::to_owned),|d,code|d.child(format!("Authorization code: {code}")))
                .when_some(url,|d,url|d.child(action("open-model-sign-in","Open sign-in").on_click(move |_,_,cx|cx.open_url(&url))))
                .when(event["kind"] == "url", |d| d
                    .child(div().text_size(px(12.)).text_color(theme.text_muted).child("If your browser cannot return to this device, copy its localhost callback address here."))
                    .child(self.callback.clone())
                    .child(action("complete-model-sign-in", "Complete sign-in").on_click(cx.listener(|this,_,_,cx| {
                        let url=this.callback.read(cx).text().to_owned();
                        let id=this.login["connectionId"].clone();
                        this.callback.update(cx,|input,cx|input.set_text("",cx));
                        if id.is_string() { this.request(methods::MODEL_COMPLETE_LOGIN,json!({"connectionId":id,"callbackUrl":url}),cx); }
                    }))))
                .child(action("cancel-model-sign-in","Cancel").on_click(cx.listener(|this,_,_,cx|this.cancel(cx))))))
            .children(connections.into_iter().enumerate().map(|(i,c)| {
                let id = c["id"].as_str().unwrap_or_default().to_owned();
                let provider = c["provider"].as_str().unwrap_or("chatgpt").to_owned();
                let editing = self.editing.as_deref() == Some(id.as_str());
                let label = c["name"].as_str().unwrap_or("Model account").to_owned();
                let button = |key: &'static str,text: &str| widgets::ghost_action(&theme).id((key,i)).child(SharedString::from(text.to_owned()));
                div().flex().flex_col().gap(px(8.)).py(px(10.)).border_t_1().border_color(theme.border)
                    .child(div().flex().items_center().gap(px(8.)).child(div().flex_1().child(label.clone()))
                        .child(format!("{} · {}{}", c["storage"].as_str().unwrap_or("local"), c["status"].as_str().unwrap_or("unknown"), if c["preferred"] == true { " · Preferred" } else { "" })))
                    .when_some(c["email"].as_str().map(str::to_owned),|d,email|d.child(div().text_size(px(12.)).text_color(theme.text_muted).child(email)))
                    .when(!busy,|d| {
                        let reconnect = id.clone(); let prefer = id.clone(); let rename = id.clone(); let disconnect = id.clone();
                        d.child(div().flex().items_center().gap(px(8.))
                            .child(button("reconnect-model","Reconnect").on_click(cx.listener(move |this,_,_,cx|this.connect(provider.clone(),Some(reconnect.clone()),cx))))
                            .when(c["authorized"] == true && c["preferred"] != true,|d|d.child(button("prefer-model","Use by default").on_click(cx.listener(move |this,_,_,cx|this.request(methods::MODEL_PREFERRED,json!({"connectionId":prefer}),cx)))))
                            .child(button("rename-model","Rename").on_click(cx.listener(move |this,_,_,cx|{this.editing=Some(rename.clone());this.name.update(cx,|input,cx|input.set_text(label.clone(),cx));cx.notify();})))
                            .child(button("disconnect-model","Disconnect").on_click(cx.listener(move |this,_,_,cx|this.request(methods::MODEL_DISCONNECT,json!({"connectionId":disconnect}),cx)))))
                    })
                    .when(editing,|d|d.child(div().flex().items_center().gap(px(8.)).child(self.name.clone())
                        .child(button("save-model-name","Save").on_click(cx.listener(move |this,_,_,cx|{let name=this.name.read(cx).text().to_owned();this.request(methods::MODEL_RENAME,json!({"connectionId":id,"name":name}),cx);})))
                        .child(button("cancel-model-name","Cancel").on_click(cx.listener(|this,_,_,cx|{this.editing=None;cx.notify();})))))
                    .when(c["authorized"] == true && c["planNoticeSeen"] != true, |d| {
                        let id=c["id"].clone();
                        d.child(div().text_size(px(12.)).text_color(theme.text_muted).child("Model access and usage limits depend on your provider plan."))
                            .child(button("acknowledge-model-plan","Got it").on_click(cx.listener(move |this,_,_,cx|this.request(methods::MODEL_ACKNOWLEDGE_PLAN,json!({"connectionId":id}),cx))))
                    })
            }));
        widgets::group_with_action(
            &theme,
            "Model connections",
            "Connect a model account for the Zuse agent. CLI agents keep their own logins.",
            Some(
                action(
                    "refresh-model-connections",
                    if self.loading {
                        "Loading…"
                    } else {
                        "Refresh"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                .into_any_element(),
            ),
        )
        .child(widgets::section_card(&theme).mt_0().child(content))
    }
}
