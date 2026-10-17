//! Browser model-account controls use the same metadata-only engine API.
use super::*;
use crate::settings_style::{self as s, button as control};
use crate::shell::{column, label, row};

impl WorkspaceView {
    pub(super) fn refresh_model_connections(&mut self) {
        if let Some(id) = self.model_fetch.take() {
            self.cancel(id);
        }
        self.model_fetch = Some(self.call(methods::MODEL_CONNECTIONS, json!({})));
    }
    pub(super) fn reset_model_connections(&mut self) {
        for id in [
            self.model_fetch.take(),
            self.model_sign_in.take(),
            self.model_action.take(),
        ]
        .into_iter()
        .flatten()
        {
            self.cancel(id);
        }
        self.model_connections = Value::Null;
        self.model_login = Value::Null;
        self.model_error = None;
        self.model_storage = "local";
        self.model_editing = None;
    }
    fn model_request(&mut self, method: &'static str, params: Value) {
        if self.model_action.is_some() {
            return;
        }
        self.model_error = None;
        self.model_action = Some(self.call(method, params));
    }
    fn connect_model(&mut self, provider: String, id: Option<String>) {
        if self.model_sign_in.is_some() || self.model_action.is_some() {
            return;
        }
        self.model_error = None;
        self.model_login = json!({"event":{"kind":"starting"}});
        self.model_sign_in = Some(self.call(
            methods::MODEL_CONNECT,
            json!({"provider":provider,"connectionId":id,"storage":self.model_storage}),
        ));
    }
    fn cancel_model_login(&mut self) {
        if let Some(id) = self.model_sign_in.take() {
            self.cancel(id);
        }
        let id = self.model_login["connectionId"].clone();
        self.model_login = Value::Null;
        if id.is_string() {
            self.model_request(methods::MODEL_CANCEL_LOGIN, json!({"connectionId":id}));
        }
    }
    pub(super) fn receive_model_accounts(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let fetch = self.model_fetch == Some(id);
        let login = self.model_sign_in == Some(id);
        let action = self.model_action == Some(id);
        if !fetch && !login && !action {
            return false;
        }
        let terminal =
            frame.get("err").is_some() || frame.get("ok").is_some() || frame["done"] == true;
        if let Some(error) = frame["err"].as_str() {
            self.model_error = Some(error.to_owned());
        }
        if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
            if fetch {
                self.model_connections = value.clone();
            }
            if login {
                self.model_login = value.clone();
                match value["event"]["kind"].as_str() {
                    Some("connected") => {
                        self.model_sign_in = None;
                        self.cancel(id);
                        self.model_login = Value::Null;
                        self.model_callback
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        self.refresh_model_connections();
                    }
                    Some("error") => {
                        self.model_error = Some(
                            value["event"]["message"]
                                .as_str()
                                .unwrap_or("Sign-in failed. Try again.")
                                .into(),
                        );
                        self.model_sign_in = None;
                        self.cancel(id);
                        self.model_login = Value::Null;
                        self.model_callback
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        self.refresh_model_connections();
                    }
                    _ => {}
                }
            }
            if action {
                if value["revoked"] == false {
                    self.model_error = Some("Disconnected here. Provider revocation could not be confirmed; you can also revoke access in your provider account.".into());
                }
                self.model_editing = None;
                self.refresh_model_connections();
            }
        }
        if terminal {
            if fetch {
                self.model_fetch = None;
            }
            if action {
                self.model_action = None;
            }
            if login && self.model_sign_in == Some(id) {
                self.model_sign_in = None;
                self.model_login = Value::Null;
                if self.model_error.is_none() {
                    self.model_error =
                        Some("Sign-in ended before authorization completed. Try again.".into());
                }
            }
            self.pending.remove(&id);
        }
        cx.notify();
        true
    }
    pub(super) fn model_accounts_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        s::frame()
            .child(
                s::header(
                    "Model connections",
                    "Connect a model account for the Zuse agent. CLI agents keep their own logins.",
                )
                .child(
                    control(
                        "refresh-model-accounts",
                        if self.model_fetch.is_some() {
                            "Loading…"
                        } else {
                            "Refresh"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_model_connections();
                        cx.notify();
                    })),
                ),
            )
            .child(s::card().child(self.model_accounts_contents(cx)))
            .into_any_element()
    }
    fn model_accounts_contents(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.model_action.is_some() || self.model_sign_in.is_some();
        let account = self.model_connections["accountAvailable"] == true;
        let local = self.model_connections["localAvailable"] == true;
        let event = &self.model_login["event"];
        let url = event["url"]
            .as_str()
            .or_else(|| event["verificationUrl"].as_str())
            .filter(|s| s.starts_with("https://"))
            .map(str::to_owned);
        column().gap_3().p_3()
            .child(row().flex_wrap().gap_2()
                .child(control("model-store-local","This device").when(self.model_storage=="local",|button|button.child(crate::assets::icon("check").size(px(12.)))).on_click(cx.listener(|this,_,_,cx|{this.model_storage="local";cx.notify();})))
                .when(account,|d|d.child(control("model-store-account","Zuse account").when(self.model_storage=="account",|button|button.child(crate::assets::icon("check").size(px(12.)))).on_click(cx.listener(|this,_,_,cx|{this.model_storage="account";cx.notify();}))))
                .when(!busy && ((self.model_storage=="local" && local)||(self.model_storage=="account" && account)),|d|d
                    .child(control("connect-chatgpt-model","Connect ChatGPT").on_click(cx.listener(|this,_,_,cx|{this.connect_model("chatgpt".into(),None);cx.notify();})))
                    .child(control("connect-supergrok-model","Connect SuperGrok").on_click(cx.listener(|this,_,_,cx|{this.connect_model("supergrok".into(),None);cx.notify();})))))
            .when(self.model_connections["accountError"]==true,|d|d.child(label("Account storage is unavailable. Connections on this device remain available.")))
            .when_some(self.model_error.clone(),|d,error|d.child(div().text_size(px(12.)).text_color(rgb(0xf87171)).child(error)))
            .when(self.model_sign_in.is_some(),|d|d.child(column().gap_2()
                .child(label(if event["kind"]=="starting" {"Starting sign-in…"} else {"Finish authorization in your browser."}))
                .when_some(event["userCode"].as_str().map(str::to_owned),|d,code|d.child(format!("Authorization code: {code}")))
                .when_some(url,|d,url|d.child(control("open-model-sign-in","Open sign-in").on_click(move |_,_,cx|cx.open_url(&url))))
                .when(event["kind"] == "url", |d| d
                    .child(label("If your browser cannot return to this engine, copy its localhost callback address here."))
                    .child(self.model_callback.clone())
                    .child(control("complete-model-sign-in","Complete sign-in").on_click(cx.listener(|this,_,_,cx| {
                        let url=this.model_callback.read(cx).value(cx);
                        let id=this.model_login["connectionId"].clone();
                        this.model_callback.update(cx,|input,cx|input.set_value(String::new(),cx));
                        if id.is_string() {this.model_request(methods::MODEL_COMPLETE_LOGIN,json!({"connectionId":id,"callbackUrl":url}));}
                        cx.notify();
                    }))))
                .child(control("cancel-model-sign-in","Cancel").on_click(cx.listener(|this,_,_,cx|{this.cancel_model_login();this.model_callback.update(cx,|input,cx|input.set_value(String::new(),cx));cx.notify();})))))
            .children(self.model_connections["connections"].as_array().into_iter().flatten().enumerate().map(|(i,c)|{
                let id=c["id"].as_str().unwrap_or_default().to_owned();
                let provider=c["provider"].as_str().unwrap_or("chatgpt").to_owned();
                let name=c["name"].as_str().unwrap_or("Model account").to_owned();
                let editing=self.model_editing.as_deref()==Some(id.as_str());
                column().gap_2().py_3().border_t_1().border_color(rgb(0x232323))
                    .child(row().gap_3().child(div().flex_1().child(name.clone()))
                        .child(label(format!("{} · {}{}",c["storage"].as_str().unwrap_or("local"),c["status"].as_str().unwrap_or("unknown"),if c["preferred"]==true {" · Preferred"} else {""}))))
                    .when_some(c["email"].as_str().map(str::to_owned),|d,email|d.child(label(email)))
                    .when(!busy,|d|{
                        let reconnect=id.clone();let prefer=id.clone();let rename=id.clone();let disconnect=id.clone();
                        d.child(row().gap_2()
                            .child(control(("reconnect-model",i),"Reconnect").on_click(cx.listener(move |this,_,_,cx|{this.connect_model(provider.clone(),Some(reconnect.clone()));cx.notify();})))
                            .when(c["authorized"]==true && c["preferred"]!=true,|d|d.child(control(("prefer-model",i),"Use by default").on_click(cx.listener(move |this,_,_,cx|{this.model_request(methods::MODEL_PREFERRED,json!({"connectionId":prefer}));cx.notify();}))))
                            .child(control(("rename-model",i),"Rename").on_click(cx.listener(move |this,_,_,cx|{this.model_editing=Some(rename.clone());this.model_name.update(cx,|input,cx|input.set_value(name.clone(),cx));cx.notify();})))
                            .child(control(("disconnect-model",i),"Disconnect").on_click(cx.listener(move |this,_,_,cx|{this.model_request(methods::MODEL_DISCONNECT,json!({"connectionId":disconnect}));cx.notify();}))))
                    })
                    .when(editing,|d|d.child(row().gap_2().child(div().flex_1().child(self.model_name.clone()))
                        .child(control(("save-model-name",i),"Save").on_click(cx.listener(move |this,_,_,cx|{let name=this.model_name.read(cx).value(cx);this.model_request(methods::MODEL_RENAME,json!({"connectionId":id,"name":name}));cx.notify();})))
                        .child(control(("cancel-model-name",i),"Cancel").on_click(cx.listener(|this,_,_,cx|{this.model_editing=None;cx.notify();})))))
                    .when(c["authorized"]==true && c["planNoticeSeen"]!=true,|d|{
                        let id=c["id"].clone();
                        d.child(label("Model access and usage limits depend on your provider plan."))
                            .child(control(("acknowledge-model-plan",i),"Got it").on_click(cx.listener(move |this,_,_,cx|{this.model_request(methods::MODEL_ACKNOWLEDGE_PLAN,json!({"connectionId":id}));cx.notify();})))
                    })
            })).into_any_element()
    }
}
