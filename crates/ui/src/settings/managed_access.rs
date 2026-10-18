//! Account-managed computer settings; credentials remain in the engine.
use super::{
    access_widgets::{details, field, frame, header},
    widgets,
};
use crate::{
    composer::ComposerInput,
    state::{AppState, EngineHandle},
    theme::Theme,
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use zuse_rpc::methods;

pub struct AccessChanged;
impl EventEmitter<AccessChanged> for ManagedAccessCard {}

pub struct ManagedAccessCard {
    state: Entity<AppState>,
    engine: Option<EngineHandle>,
    account: Value,
    host: Value,
    environments: Vec<Value>,
    saved: Vec<Value>,
    devices: Vec<Value>,
    name: Entity<ComposerInput>,
    error: Option<String>,
    busy: bool,
    epoch: u64,
    watch: Option<Task<()>>,
    status_watch: Option<Task<()>>,
    load: Option<Task<()>>,
    action: Option<Task<()>>,
    _state: Subscription,
}
impl ManagedAccessCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let engine = state.read(cx).engine().cloned();
        let observe = cx.observe(&state, |this: &mut Self, _, cx| {
            let next = this.state.read(cx).engine().cloned();
            let same = match (&this.engine, &next) {
                (Some(a), Some(b)) => a.same_connection(b),
                (None, None) => true,
                _ => false,
            };
            if same {
                return;
            }
            this.engine = next;
            this.epoch = this.epoch.wrapping_add(1);
            this.watch = None;
            this.status_watch = None;
            this.load = None;
            this.action = None;
            this.busy = false;
            this.account = Value::Null;
            this.host = Value::Null;
            this.environments.clear();
            this.saved.clear();
            this.devices.clear();
            this.error = None;
            this.attach(cx);
            cx.notify();
        });
        let name = cx.new(|cx| ComposerInput::new("Computer name", cx));
        let mut card = Self {
            state,
            engine,
            account: Value::Null,
            host: Value::Null,
            environments: vec![],
            saved: vec![],
            devices: vec![],
            name,
            error: None,
            busy: false,
            epoch: 0,
            watch: None,
            status_watch: None,
            load: None,
            action: None,
            _state: observe,
        };
        card.attach(cx);
        card
    }
    fn attach(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let status_engine = engine.clone();
        self.status_watch = Some(cx.spawn(async move |this, cx| {
            match status_engine
                .client()
                .subscribe_scoped(methods::WATCH_MANAGED_HOST, json!({}))
                .await
            {
                Ok(mut stream) => {
                    while let Some(value) = stream.recv().await {
                        if this
                            .update(cx, |card, cx| {
                                if !card
                                    .engine
                                    .as_ref()
                                    .is_some_and(|e| e.same_connection(&status_engine))
                                {
                                    return;
                                }
                                card.host = value;
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
                        if card
                            .engine
                            .as_ref()
                            .is_some_and(|e| e.same_connection(&status_engine))
                        {
                            card.error = Some(error.to_string());
                            cx.notify();
                        }
                    });
                }
            }
        }));
        self.watch = Some(cx.spawn(async move |this, cx| {
            match engine
                .client()
                .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await
            {
                Ok(mut stream) => {
                    while let Some(value) = stream.recv().await {
                        if this
                            .update(cx, |card, cx| {
                                if !card
                                    .engine
                                    .as_ref()
                                    .is_some_and(|current| current.same_connection(&engine))
                                {
                                    return;
                                }
                                let changed = card.account.is_null()
                                    || card.account["user"]["id"] != value["user"]["id"];
                                card.account = value;
                                if changed {
                                    card.epoch = card.epoch.wrapping_add(1);
                                    card.load = None;
                                    card.action = None;
                                    card.busy = false;
                                    card.environments.clear();
                                    card.saved.clear();
                                    card.devices.clear();
                                    card.error = None;
                                    card.refresh(cx);
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
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.epoch = self.epoch.wrapping_add(1);
        let epoch = self.epoch;
        let signed_in = self.account["user"].is_object();
        self.load = Some(cx.spawn(async move |this, cx| {
            if signed_in {
                let saved = engine
                    .client()
                    .call(methods::LIST_ACCOUNT_ROUTES, json!({}))
                    .await;
                let _ = this.update(cx, |card, cx| {
                    if card.epoch == epoch {
                        match saved {
                            Ok(value) => {
                                card.saved =
                                    value["computers"].as_array().cloned().unwrap_or_default()
                            }
                            Err(error) => card.error = Some(error.to_string()),
                        }
                        cx.notify();
                    }
                });
            }
            let catalog = async {
                if !signed_in {
                    return None;
                }
                Some(futures::join!(
                    engine
                        .client()
                        .call(methods::LIST_ACCOUNT_ENVIRONMENTS, json!({})),
                    engine
                        .client()
                        .call(methods::LIST_ACCOUNT_DEVICES, json!({}))
                ))
            };
            let catalog = catalog.await;
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                if let Some((environments, devices)) = catalog {
                    match environments {
                        Ok(value) => {
                            card.environments = value["environments"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default()
                        }
                        Err(e) => card.error = Some(e.to_string()),
                    }
                    match devices {
                        Ok(value) => {
                            card.devices = value["clients"].as_array().cloned().unwrap_or_default()
                        }
                        Err(e) => card.error = Some(e.to_string()),
                    }
                }
                cx.notify();
            });
        }));
    }
    fn request(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.load = None;
        self.epoch = self.epoch.wrapping_add(1);
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
                let forgetting = method == methods::FORGET_ACCOUNT_ROUTE;
                match result {
                    Ok(value) if method == methods::CLOUD_ACCOUNT_SIGN_IN => {
                        if let Some(url) = value["url"]
                            .as_str()
                            .filter(|url| url.starts_with("https://"))
                        {
                            cx.open_url(url);
                        }
                    }
                    Ok(value) if forgetting => {
                        card.saved = value["computers"].as_array().cloned().unwrap_or_default()
                    }
                    Err(error) => card.error = Some(error.to_string()),
                    _ => {}
                }
                if !forgetting
                    && !matches!(
                        method,
                        methods::CLOUD_ACCOUNT_SIGN_IN | methods::CLOUD_ACCOUNT_SIGN_OUT
                    )
                {
                    card.refresh(cx);
                    if matches!(
                        method,
                        methods::LINK_MANAGED_HOST | methods::UNLINK_MANAGED_HOST
                    ) {
                        cx.emit(AccessChanged);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Render for ManagedAccessCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let signed_in = self.account["user"].is_object();
        let configured = self.host["configured"] == true;
        let title = if !configured {
            "Link this computer"
        } else if self.host["enabled"] != true {
            "Remote access disabled"
        } else if self.host["accountMatches"] != true {
            "Sign in to the linked account"
        } else if self.host["tunnelRunning"] == true {
            "Remote access started"
        } else {
            "Starting remote access"
        };
        let description = if configured {
            "Connect from your other signed-in computers, or share a private invitation below."
        } else {
            "Link your account, then create a private invitation below to connect another device."
        };
        let busy = self.busy;
        div().flex().flex_col().gap(px(20.)).mt(px(24.))
            .child(frame(&theme).child(header(&theme,"Zuse Serve","Connect your computers across networks using your Zuse account."))
                .child(widgets::section_card(&theme).mt(px(0.))
                    .child(widgets::card_row(&theme,true).child(details(&theme,"Zuse account",&self.account["user"]["email"].as_str().map(zuse_protocol::presentation::masked_email).unwrap_or_else(|| "Sign in to link this computer.".into())))
                        .child(widgets::text_action(&theme,widgets::ActionTone::Outlined,if busy {"Working…"}else if signed_in {"Sign out"}else{"Sign in"}).id("managed-account-sign-in")
                            .on_click(cx.listener(move|this,_,_,cx|this.request(if signed_in{methods::CLOUD_ACCOUNT_SIGN_OUT}else{methods::CLOUD_ACCOUNT_SIGN_IN},json!({}),cx)))))
                    .when_some(self.account["login"]["url"].as_str().filter(|url|url.starts_with("https://")).map(str::to_owned),|card,url|card.child(widgets::card_row(&theme,false)
                        .child(details(&theme,"Complete sign-in",self.account["login"]["userCode"].as_str().unwrap_or("Continue in your browser.")))
                        .child(widgets::text_action(&theme,widgets::ActionTone::Outlined,"Open sign-in").id("managed-open-sign-in").on_click(move|_,_,cx|cx.open_url(&url)))))
                    .child(widgets::card_row(&theme,false).child(details(&theme,title,description))
                        .when(signed_in || configured,|row|row.child(widgets::text_action(&theme,widgets::ActionTone::Outlined,if busy {"Working…"}else if configured{"Unlink computer"}else{"Link computer"}).id("managed-host-link")
                            .on_click(cx.listener(move|this,_,_,cx|{
                                let method=if configured{methods::UNLINK_MANAGED_HOST}else{methods::LINK_MANAGED_HOST};
                                let label=this.name.read(cx).text().trim().to_owned();
                                this.request(method,json!({"label":label}),cx);
                            }))))
                    .when(signed_in && !configured,|card|card.child(widgets::card_row(&theme,false).child(details(&theme,"Computer name","A name you recognize on your other devices.")).child(field(&theme,self.name.clone()))))
                    .when_some(self.error.as_deref().or(self.host["error"].as_str()).or(self.account["error"].as_str()),|card,error|card.child(widgets::card_row(&theme,false).child(div().text_size(px(12.)).text_color(theme.danger).child(error.to_owned()))))
                    .child(widgets::card_row(&theme,false).child(widgets::text_action(&theme,widgets::ActionTone::Quiet,"Refresh remote access").id("refresh-managed-access").on_click(cx.listener(|this,_,_,cx|{if !this.busy {this.error=None;this.refresh(cx);cx.notify();}}))))))
            .when(signed_in,|view|view
                .child(frame(&theme).child(header(&theme,"Account computers","Open a linked computer in its own Zuse window."))
                    .child(widgets::section_card(&theme).mt(px(0.))
                        .when(self.environments.is_empty() && self.saved.is_empty(),|card|card.child(widgets::card_row(&theme,true).child(details(&theme,"No linked computers","Link this computer to get started."))))
                        .children(zuse_protocol::managed_connection::computer_rows(&self.environments,&self.saved).into_iter().enumerate().map(|(index,computer)|{
                            let environment=computer["environmentId"].as_str().unwrap_or_default().to_owned();
                            let subject=self.account["user"]["id"].as_str().unwrap_or_default().to_owned();
                            let controller=self.engine.clone();
                            let label=computer["label"].as_str().unwrap_or("Linked computer").to_owned();
                            let current=controller.as_ref().is_some_and(|engine|engine.engine_info().device_id==environment);
                            let compatible=zuse_protocol::managed_connection::supports_record(computer);
                            let remembered=self.saved.iter().any(|row|row["environmentId"]==computer["environmentId"]);
                            let forget_id=environment.clone();
                            let forget_subject=subject.clone();
                            widgets::card_row(&theme,index==0).child(details(&theme,computer["label"].as_str().unwrap_or("Linked computer"),if current {"This computer"}else if compatible {computer["serviceState"].as_str().unwrap_or("Availability checked on connect")}else{"Update Zuse on this computer to connect through your account."}))
                                .when(!current && compatible,|row|row.child(widgets::text_action(&theme,widgets::ActionTone::Outlined,"Connect").id(("connect-account-computer",index))
                                    .on_click(move|_,_,cx|{if let Some(controller)=controller.clone(){crate::open_account_window(crate::state::AccountTarget{controller,subject:subject.clone(),environment:environment.clone(),label:label.clone()},cx);}})))
                                .when(remembered,|row|row.child(widgets::text_action(&theme,widgets::ActionTone::Quiet,"Forget saved").id(("forget-account-computer",index)).on_click(cx.listener(move|this,_,_,cx|this.request(methods::FORGET_ACCOUNT_ROUTE,json!({"environmentId":forget_id,"expectedSubject":forget_subject}),cx)))))
                        }))))
                .child(frame(&theme).child(header(&theme,"Authorized account devices","Revoke a device that no longer needs account access."))
                    .child(widgets::section_card(&theme).mt(px(0.))
                        .when(self.devices.is_empty(),|card|card.child(widgets::card_row(&theme,true).child(details(&theme,"No authorized devices","Devices appear here after they connect through your account."))))
                        .children(self.devices.iter().enumerate().map(|(index,device)|{
                            let id=device["clientId"].as_str().unwrap_or_default().to_owned();
                            widgets::card_row(&theme,index==0).child(details(&theme,device["label"].as_str().unwrap_or("Account device"),device["platform"].as_str().unwrap_or("Device")))
                                .child(widgets::text_action(&theme,widgets::ActionTone::Quiet,"Revoke").id(("revoke-account-device",index)).on_click(cx.listener(move|this,_,_,cx|this.request(methods::REVOKE_ACCOUNT_DEVICE,json!({"clientId":id}),cx))))
                        })))))
            )
    }
}
