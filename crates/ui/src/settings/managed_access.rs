//! Account-managed computer settings; credentials remain in the engine.
use super::{access_widgets::field, widgets};
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
    dialog: Option<Dialog>,
    dialog_focus: FocusHandle,
    dialog_root: FocusHandle,
    return_focus: Option<FocusHandle>,
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
            this.dialog = None;
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
            dialog: None,
            dialog_focus: cx.focus_handle().tab_stop(true),
            dialog_root: cx.focus_handle(),
            return_focus: None,
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
        let params = if method == methods::CLOUD_ACCOUNT_SIGN_IN {
            engine.account_sign_in_params()
        } else {
            params
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
                if result.is_ok()
                    && matches!(
                        method,
                        methods::LINK_MANAGED_HOST
                            | methods::UNLINK_MANAGED_HOST
                            | methods::REVOKE_ACCOUNT_DEVICE
                    )
                {
                    card.dialog = None;
                }
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
#[derive(Clone)]
enum Dialog {
    Setup,
    Unlink,
}

impl ManagedAccessCard {
    fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.dialog = Some(dialog);
        self.return_focus = window.focused(cx);
        self.error = None;
        window.focus(&self.dialog_focus, cx);
        cx.notify();
    }
    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.dialog = None;
        if let Some(focus) = self.return_focus.take() {
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        cx.notify();
    }
    pub(super) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.dialog.is_none() {
            return false;
        }
        if !self.busy {
            self.dialog = None;
            cx.notify();
        }
        true
    }
    pub(super) fn method(&self, cx: &mut Context<Self>) -> AnyElement {
        use zuse_viewport::remote_access_style as r;
        let configured = self.host["configured"] == true;
        let online = self.host["tunnelRunning"] == true;
        let description = if configured && online {
            format!(
                "Available anywhere · {}",
                self.host["endpoint"]["httpBaseUrl"]
                    .as_str()
                    .unwrap_or("Zuse account")
            )
        } else if configured && self.host["accountMatches"] != true {
            "Sign in to the linked account.".into()
        } else if configured {
            "Reconnecting to your account…".into()
        } else {
            "Access this computer from your signed-in devices.".into()
        };
        r::method_row("remote-server", "Zuse Serve", description, true)
            .child(
                r::method_button(
                    "managed-host-link",
                    if self.busy {
                        "Updating…"
                    } else if configured {
                        "Turn off"
                    } else {
                        "Set up"
                    },
                    cx,
                )
                .when(self.busy, |v| v.opacity(0.5))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_dialog(
                        if configured {
                            Dialog::Unlink
                        } else {
                            Dialog::Setup
                        },
                        window,
                        cx,
                    )
                })),
            )
            .into_any_element()
    }
    pub(super) fn computers(&self, add: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        use zuse_viewport::{remote_access_style as r, settings_style as s};
        let rows =
            zuse_protocol::managed_connection::computer_rows(&self.environments, &self.saved);
        r::frame(crate::localization::text("settings-remote_other_computers", cx),crate::localization::text("settings-using_computers_card_projects_and_chats_from_connected_computers_stay_in_the_same_sidebar", cx),Some(add))
            .child(s::card().when(rows.is_empty(),|v|v.child(r::empty_computers(cx)))
                .children(rows.into_iter().enumerate().map(|(index,computer)| {
                    let environment=computer["environmentId"].as_str().unwrap_or_default().to_owned();
                    let subject=self.account["user"]["id"].as_str().unwrap_or_default().to_owned();
                    let controller=self.engine.clone();let label=computer["label"].as_str().unwrap_or("Linked computer").to_owned();
                    let current=controller.as_ref().is_some_and(|engine|engine.engine_info().device_id==environment);
                    let compatible=zuse_protocol::managed_connection::supports_record(computer);
                    let remembered=self.saved.iter().any(|row|row["environmentId"]==computer["environmentId"]);
                    let forget_id=environment.clone();let forget_subject=subject.clone();
                    let description=if current {"This computer"}else if compatible {computer["serviceState"].as_str().unwrap_or("Availability checked on connect")}else{"Update Zuse on this computer to connect."};
                    div().flex().items_center().min_h(px(48.)).gap(px(10.)).px(px(12.)).py(px(8.))
                        .when(index>0,|v|v.border_t_1().border_color(s::card_border().opacity(0.4)))
                        .child(zuse_viewport::remote_access_style::glyph("monitor").size(px(16.)))
                        .child(r::details(label.clone(),description.to_owned()).flex_1())
                        .when(!current&&compatible,|v|v.child(r::button(("connect-account-computer",index),crate::localization::text("settings-remote_connect", cx))
                            .on_click(move|_,_,cx|if let Some(controller)=controller.clone(){crate::open_account_window(crate::state::AccountTarget{controller,subject:subject.clone(),environment:environment.clone(),label:label.clone()},cx);})))
                        .when(remembered,|v|v.child(r::button(("forget-account-computer",index),"Forget saved")
                            .on_click(cx.listener(move|this,_,_,cx|this.request(methods::FORGET_ACCOUNT_ROUTE,json!({"environmentId":forget_id,"expectedSubject":forget_subject}),cx)))))
                }))).into_any_element()
    }
    pub(super) fn account_devices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use zuse_viewport::{remote_access_style as r, settings_style as s};
        if !self.account["user"].is_object() {
            return None;
        }
        Some(
            r::frame(
                "Account devices",
                "Manage devices authorized through your account.",
                None,
            )
            .child(
                s::card()
                    .when(self.devices.is_empty(), |v| {
                        v.child(r::empty("No account devices connected yet."))
                    })
                    .children(self.devices.iter().enumerate().map(|(index, device)| {
                        let id = device["clientId"].as_str().unwrap_or_default().to_owned();
                        let label = device["label"]
                            .as_str()
                            .unwrap_or("Account device")
                            .to_owned();
                        div()
                            .flex()
                            .items_center()
                            .min_h(px(48.))
                            .gap(px(10.))
                            .px(px(12.))
                            .py(px(8.))
                            .when(index > 0, |v| {
                                v.border_t_1().border_color(s::card_border().opacity(0.4))
                            })
                            .child(
                                r::details(
                                    label.clone(),
                                    device["platform"].as_str().unwrap_or("Device").to_owned(),
                                )
                                .flex_1(),
                            )
                            .child(
                                r::small_button(
                                    ("revoke-account-device", index),
                                    crate::localization::text("settings-organizations_revoke", cx),
                                )
                                .border_color(gpui::transparent_black())
                                .text_color(zuse_viewport::settings_style::destructive_text())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.request(
                                            methods::REVOKE_ACCOUNT_DEVICE,
                                            json!({"clientId":id}),
                                            cx,
                                        );
                                        cx.notify();
                                    },
                                )),
                            )
                    })),
            )
            .into_any_element(),
        )
    }
    pub(super) fn dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use zuse_viewport::remote_access_style as r;
        let dialog = self.dialog.clone()?;
        let theme = Theme::of(cx).for_settings_surface();
        let signed_in = self.account["user"].is_object();
        let (title, description) = match &dialog {
            Dialog::Setup => (
                "Set up Zuse Serve",
                "Link this computer to your Zuse account to access it from your other devices.",
            ),
            Dialog::Unlink => (
                "Turn off Zuse Serve?",
                "This computer will no longer be available through your account. Local chats stay on this computer.",
            ),
        };
        let mut body = r::dialog_body();
        match &dialog {
            Dialog::Setup => {
                body = body
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                r::details(
                                    crate::localization::text(
                                        "settings-model_connections_account_scope",
                                        cx,
                                    ),
                                    self.account["user"]["email"]
                                        .as_str()
                                        .map(zuse_protocol::presentation::masked_email)
                                        .unwrap_or_else(|| "Sign in to link this computer.".into()),
                                )
                                .flex_1(),
                            )
                            .child(
                                r::button(
                                    "managed-account-sign-in",
                                    if self.busy {
                                        crate::localization::text(
                                            "settings-model_connections_working",
                                            cx,
                                        )
                                    } else if signed_in {
                                        crate::localization::text("common-signOut", cx)
                                    } else {
                                        crate::localization::text("settings-acp_agents_sign_in", cx)
                                    },
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.request(
                                            if signed_in {
                                                methods::CLOUD_ACCOUNT_SIGN_OUT
                                            } else {
                                                methods::CLOUD_ACCOUNT_SIGN_IN
                                            },
                                            json!({}),
                                            cx,
                                        )
                                    },
                                )),
                            ),
                    )
                    .when_some(
                        self.account["login"]["url"]
                            .as_str()
                            .filter(|url| url.starts_with("https://"))
                            .map(str::to_owned),
                        |body, url| {
                            body.child(r::details(
                                "Complete sign-in",
                                self.account["login"]["userCode"]
                                    .as_str()
                                    .unwrap_or("Continue in your browser.")
                                    .to_owned(),
                            ))
                            .child(
                                r::button("managed-open-sign-in", "Open sign-in")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        },
                    )
                    .when(signed_in, |body| {
                        body.child(r::details(
                            "Computer name",
                            "A name you recognize on your other devices.",
                        ))
                        .child(field(&theme, self.name.clone()))
                    });
            }
            _ => {}
        }
        body = body.when_some(
            self.error
                .as_deref()
                .or(self.host["error"].as_str())
                .or(self.account["error"].as_str()),
            |body, error| body.child(widgets::error_strip(&theme, error.to_owned())),
        );
        let confirm = if self.busy {
            "Updating…"
        } else {
            match dialog {
                Dialog::Setup => "Link computer",
                Dialog::Unlink => "Turn off",
            }
        };
        let enabled = !self.busy && (!matches!(dialog, Dialog::Setup) || signed_in);
        Some(r::Modal(r::dialog_panel("managed-access-dialog",title,description).track_focus(&self.dialog_root)
            .on_key_down(cx.listener(|this,event:&KeyDownEvent,window,cx|if event.keystroke.key=="escape"{this.close_dialog(window,cx);cx.stop_propagation();}))
            .child(body).child(r::dialog_footer()
                .child(r::button("close-managed-access",crate::localization::text("settings-organizations_cancel", cx)).track_focus(&self.dialog_focus).on_click(cx.listener(|this,_,window,cx|this.close_dialog(window,cx))))
                .child(r::button("confirm-managed-access",confirm).when(!enabled,|v|v.opacity(0.5).cursor_default())
                    .on_click(cx.listener(move|this,_,_,cx|if enabled {
                        match &dialog {
                            Dialog::Setup=>this.request(methods::LINK_MANAGED_HOST,json!({"label":this.name.read(cx).text().trim()}),cx),
                            Dialog::Unlink=>this.request(methods::UNLINK_MANAGED_HOST,json!({}),cx),
                        }
                    })))).into_any_element()).into_any_element())
    }
}
