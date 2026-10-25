//! Provider setup follows Zuse's expandable rows and canonical engine actions.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BORDER, MUTED, column, provider_icon, row},
};
impl WorkspaceView {
    fn provider_account_action(&mut self, provider: &str, event: &crate::provider_accounts::Event, cx: &mut Context<Self>) {
        use crate::provider_accounts::Action;
        if event.scope != self.connection_epoch.to_string() { return; }
        let id=match &event.action { Action::Switch(id)|Action::Remove(id)=>id };
        let account=self.accounts["accounts"].as_array().into_iter().flatten().find(|a|a["id"]==*id&&a["harness"]==provider).cloned();
        let Some(account)=account else{
            if let Some(rows)=self.provider_account_rows.get(provider){rows.update(cx,|rows,cx|rows.finish(Err("Refresh accounts before trying again.".into()),cx));}
            return;
        };
        let provider=account["harness"].as_str().unwrap_or_default().to_owned();
        let login_pending=self.provider_logins.get(&provider).is_some_and(|login|login["cancelled"]!=true&&matches!(login["status"].as_str(),Some("starting"|"pending")));
        if !self.connected || self.provider_actions.values().any(|p|p==&provider) || login_pending || account["switchable"]!=true {
            if let Some(rows)=self.provider_account_rows.get(&provider) { rows.update(cx,|rows,cx|rows.finish(Err("Reconnect and wait for the current account action to finish.".into()),cx)); }
            return;
        }
        let method=match event.action {Action::Switch(_)=>methods::ACTIVATE_AGENT_ACCOUNT,Action::Remove(_)=>methods::FORGET_AGENT_ACCOUNT};
        self.provider_action(provider.clone(),method,json!({"id":id,"accountId":id,"harness":provider}));
        cx.notify();
    }
    pub(super) fn provider_action(
        &mut self,
        provider: String,
        method: &'static str,
        params: Value,
    ) {
        if !self.connected {
            self.provider_error =
                Some("Reconnect to this computer before changing provider setup.".into());
            return;
        }
        if self
            .provider_actions
            .values()
            .any(|pending| pending == &provider)
        {
            return;
        }
        self.provider_error = None;
        let id = self.call(method, params);
        self.provider_actions.insert(id, provider);
    }
    pub(super) fn interrupt_provider_logins(&mut self, cx: &mut Context<Self>) {
        self.provider_login_tasks.clear();
        self.provider_logins.clear();
        for input in self.provider_codes.values() {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
        self.provider_account_events.clear();
        self.provider_account_rows.clear();
        self.provider_options.clear();
    }
    fn start_provider_login(
        &mut self,
        provider: String,
        selected: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.connected || self.provider_actions.values().any(|p| p == &provider) {
            return;
        }
        self.provider_login_tasks.remove(&provider);
        if let Some(input) = self.provider_codes.get(&provider) {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
        self.provider_logins.insert(
            provider.clone(),
            json!({"status":"starting","provider":selected}),
        );
        let mut params = json!({"harness":provider});
        if let Some(selected) = selected {
            params["provider"] = json!(selected);
        }
        self.provider_action(provider, methods::START_AGENT_LOGIN, params);
        cx.notify();
    }
    fn cancel_provider_login(&mut self, provider: &str, cx: &mut Context<Self>) {
        self.provider_login_tasks.remove(provider);
        let Some(login) = self.provider_logins.get_mut(provider) else {
            return;
        };
        login["cancelled"] = json!(true);
        let id = login["loginId"].as_str().map(str::to_owned);
        if let Some(input) = self.provider_codes.get(provider) {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
        if let Some(login_id) = id {
            let request = self.call(methods::CANCEL_AGENT_LOGIN, json!({"loginId":login_id}));
            self.provider_actions.insert(request, provider.to_owned());
        }
        cx.notify();
    }
    fn complete_provider_login(&mut self, provider: &str, cx: &mut Context<Self>) {
        let Some(login) = self
            .provider_logins
            .get(provider)
            .filter(|login| login["cancelled"] != true && login["mode"] == "paste-code")
        else {
            return;
        };
        let Some(login_id) = login["loginId"].as_str().map(str::to_owned) else {
            return;
        };
        let Some(input) = self.provider_codes.get(provider) else {
            return;
        };
        let code = input.read(cx).value(cx);
        if code.trim().is_empty() {
            return;
        }
        self.provider_action(
            provider.to_owned(),
            methods::COMPLETE_AGENT_LOGIN,
            json!({"loginId":login_id,"code":code}),
        );
        cx.notify();
    }
    fn inline_provider_action(
        &mut self,
        action: crate::provider_signin::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::provider_signin::Kind;
        if !matches!(action.kind, Kind::Complete) {
            window.focus(&self.workspace_focus, cx);
        }
        let provider = String::from(action.harness);
        match action.kind {
            Kind::Start(selected) => {
                self.start_provider_login(provider, selected.map(str::to_owned), cx)
            }
            Kind::Retry => {
                let selected = self
                    .provider_logins
                    .get(&provider)
                    .and_then(|v| v["provider"].as_str())
                    .map(str::to_owned);
                self.start_provider_login(provider, selected, cx);
            }
            Kind::Cancel => self.cancel_provider_login(&provider, cx),
            Kind::Complete => self.complete_provider_login(&provider, cx),
            Kind::Open => {
                if let Some(url) = self
                    .provider_logins
                    .get(&provider)
                    .and_then(|v| v["url"].as_str())
                    .filter(|url| crate::provider_signin::is_openable_login_url(url))
                {
                    cx.open_url(url);
                }
            }
        }
    }
    fn schedule_provider_poll(
        &mut self,
        provider: String,
        login_id: String,
        cx: &mut Context<Self>,
    ) {
        let epoch = self.connection_epoch;
        let key = provider.clone();
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                let keep = this
                    .update(cx, |view, cx| {
                        if !view.connected
                            || view.connection_epoch != epoch
                            || !view.provider_logins.get(&provider).is_some_and(|login| {
                                login["loginId"] == login_id
                                    && login["status"] == "pending"
                                    && login["cancelled"] != true
                            })
                        {
                            return false;
                        }
                        if !view
                            .provider_actions
                            .values()
                            .any(|pending| pending == &provider)
                        {
                            view.provider_action(
                                provider.clone(),
                                methods::POLL_AGENT_LOGIN,
                                json!({"loginId":login_id}),
                            );
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        });
        self.provider_login_tasks.insert(key, task);
    }
    pub(super) fn dismiss_provider_login(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(provider) = self.provider_expanded.clone().filter(|provider| {
            self.provider_logins.get(provider).is_some_and(|login| {
                login["cancelled"] != true
                    && matches!(login["status"].as_str(), Some("starting" | "pending"))
            })
        }) else {
            return false;
        };
        self.cancel_provider_login(&provider, cx);
        true
    }
    pub(super) fn receive_provider_action(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(provider) = self.provider_actions.get(&id).cloned() else {
            return false;
        };
        if frame.get("ok").is_none() && frame.get("err").is_none() {
            return true;
        }
        let method = self.pending.get(&id).cloned().unwrap_or_default();
        self.provider_actions.remove(&id);
        let is_login = matches!(
            method.as_str(),
            methods::START_AGENT_LOGIN
                | methods::POLL_AGENT_LOGIN
                | methods::COMPLETE_AGENT_LOGIN
                | methods::CANCEL_AGENT_LOGIN
        );
        if matches!(method.as_str(),methods::ACTIVATE_AGENT_ACCOUNT|methods::FORGET_AGENT_ACCOUNT) {
            let result=if let Some(error)=frame["err"].as_str(){Err(error.to_owned())}
                else {serde_json::from_value::<zuse_protocol::AgentAccountsSnapshot>(frame["ok"].clone()).map(|_|{self.accounts=frame["ok"].clone();}).map_err(|_|"Unable to read the updated account list. Refresh before trying again.".to_owned())};
            if let Some(rows)=self.provider_account_rows.get(&provider){rows.update(cx,|rows,cx|rows.finish(result,cx));}
            self.pending.remove(&id);cx.notify();return true;
        }
        if let Some(error) = frame["err"].as_str() {
            if is_login {
                self.provider_login_tasks.remove(&provider);
                if let Some(login) = self.provider_logins.get_mut(&provider) {
                    if login["cancelled"] == true && method == methods::START_AGENT_LOGIN {
                        self.provider_logins.remove(&provider);
                    } else if method == methods::COMPLETE_AGENT_LOGIN && login["cancelled"] != true
                    {
                        login["error"] = json!(error);
                    } else if login["cancelled"] != true || method == methods::CANCEL_AGENT_LOGIN {
                        login["status"] = json!("failed");
                        login["message"] = json!(error);
                        login["cancelled"] = json!(false);
                    }
                }
            } else if provider.starts_with("acp-") {
                self.acp_provider_error(error.to_owned());
            } else {
                self.provider_error = Some(error.to_owned());
            }
            self.pending.remove(&id);
            cx.notify();
            return true;
        }
        let value = &frame["ok"];
        if matches!(
            method.as_str(),
            methods::SET_HARNESS_ENABLED | methods::INSTALL_HARNESS
        ) {
            return false;
        }
        let cancelled = self
            .provider_logins
            .get(&provider)
            .is_some_and(|login| login["cancelled"] == true);
        match method.as_str() {
            methods::START_AGENT_LOGIN => {
                if cancelled {
                    if let Some(login_id) = value["loginId"].as_str() {
                        let request =
                            self.call(methods::CANCEL_AGENT_LOGIN, json!({"loginId":login_id}));
                        self.provider_actions.insert(request, provider.clone());
                    } else {
                        self.provider_logins.remove(&provider);
                    }
                } else if self.provider_logins.contains_key(&provider) {
                    let selected = self.provider_logins[&provider]["provider"].clone();
                    let mut login = value.clone();
                    login["status"] = json!("pending");
                    login["provider"] = selected;
                    if let Some(url) = login["url"]
                        .as_str()
                        .filter(|url| crate::provider_signin::is_openable_login_url(url))
                    {
                        cx.open_url(url);
                    }
                    let browser = login["mode"] == "browser";
                    let login_id = login["loginId"].as_str().map(str::to_owned);
                    self.provider_logins.insert(provider.clone(), login);
                    if browser {
                        if let Some(login_id) = login_id {
                            self.schedule_provider_poll(provider.clone(), login_id, cx);
                        }
                    }
                }
            }
            methods::POLL_AGENT_LOGIN if !cancelled => {
                if let Some(login) = self.provider_logins.get_mut(&provider) {
                    if let Some(url) = value["url"]
                        .as_str()
                        .filter(|url| crate::provider_signin::is_openable_login_url(url))
                    {
                        if login["url"] != url {
                            cx.open_url(url);
                            login["url"] = json!(url);
                        }
                    }
                    if let Some(message) = value["message"].as_str() {
                        login["message"] = json!(message);
                    }
                    if value["status"] == "done" {
                        login["status"] = json!("done");
                        self.provider_login_tasks.remove(&provider);
                        self.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":true}));
                        self.call(methods::LIST_HARNESSES, json!({}));
                    } else if value["status"] == "error" {
                        login["status"] = json!("failed");
                        login["message"] = value["message"]
                            .as_str()
                            .unwrap_or("Sign-in failed.")
                            .into();
                        self.provider_login_tasks.remove(&provider);
                    }
                }
            }
            methods::COMPLETE_AGENT_LOGIN
                if !cancelled && self.provider_logins.contains_key(&provider) =>
            {
                self.provider_login_tasks.remove(&provider);
                if let Some(login) = self.provider_logins.get_mut(&provider) {
                    login["status"] = json!("done");
                    login.as_object_mut().map(|object| object.remove("error"));
                }
                if let Some(input) = self.provider_codes.get(&provider) {
                    input.update(cx, |input, cx| input.set_value(String::new(), cx));
                }
                self.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":true}));
                self.call(methods::LIST_HARNESSES, json!({}));
            }
            methods::CANCEL_AGENT_LOGIN => {
                self.provider_logins.remove(&provider);
                self.provider_login_tasks.remove(&provider);
                self.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":false}));
            }
            _ => {}
        }
        self.pending.remove(&id);
        cx.notify();
        true
    }
    pub(super) fn providers_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let refresh = row()
            .gap(px(6.))
            .child(s::description_text("Checked").text_size(px(10.)))
            .child(
                crate::shell::icon_button(
                    "refresh-provider-status",
                    "refresh",
                    "Refresh provider status",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.call(methods::LIST_HARNESSES, json!({}));
                    this.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":true}));
                    this.check_provider_updates(cx);
                    this.refresh_custom_acp(cx);
                    cx.notify();
                })),
            );
        let rows =
            self.providers
                .iter()
                .enumerate()
                .filter(|(_, provider)| provider["id"] != "zuse")
                .filter(|(_, provider)|!provider["id"].as_str().is_some_and(|id|id.starts_with("acp-")))
                .map(|(index, provider)| {
                    let id = provider["id"].as_str().unwrap_or_default().to_owned();
                    let name = provider["name"].as_str().unwrap_or("Agent").to_owned();
                    let installed = provider["installed"] != false;
                    let enabled = provider["enabled"].as_bool().unwrap_or(installed);
                    let busy = self.provider_actions.values().any(|pending| pending == &id);
                    let can_expand = enabled || !installed;
                    let open = can_expand && self.provider_expanded.as_deref() == Some(&id);
                    let account = self.accounts["accounts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|account| account["harness"] == id && account["active"] == true);
                    let status = if !enabled && installed {
                        "Disabled"
                    } else if !installed {
                        "Not installed"
                    } else if account.is_some() {
                        "Signed in"
                    } else {
                        "Installed"
                    };
                    let summary = column()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(self.provider_update_title(&id,name.clone(),index,cx))
                        .child(
                            row()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(rgb(if account.is_some() { 0x89ca21 } else { MUTED })),
                                )
                                .child(s::description_text(status).line_height(px(14.85)))
                                .when_some(
                                    account
                                        .and_then(|account| account["email"].as_str())
                                        .map(str::to_owned),
                                    |row, email| row.child(s::description_text(zuse_protocol::presentation::masked_email(&email)).line_height(px(14.85))),
                                ),
                        );
                    let toggle_provider = id.clone();
                    let toggle = s::toggle(
                        ("enable-provider", index),
                        enabled,
                        &format!("Enable {name}"),
                    )
                    .when(!installed || busy, |view| view.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if installed && !busy {
                            if enabled { this.provider_expanded = None; }
                            this.provider_action(
                                toggle_provider.clone(),
                                methods::SET_HARNESS_ENABLED,
                                json!({"harness":toggle_provider,"enabled":!enabled}),
                            );
                            cx.notify();
                        }
                    }));
                    let expand = id.clone();
                    let header = row()
                        .id(("provider-settings-row", index))
                        .min_h(px(48.))
                        .gap(px(12.))
                        .px(px(12.))
                        .py(px(8.))
                        .when(can_expand, |view| view.cursor_pointer().hover(|v| v.bg(s::frame_bg().opacity(0.3))))
                        .child(
                            row()
                                .size(px(28.))
                                .justify_center()
                                .rounded(px(7.))
                                .bg(s::frame_bg().opacity(0.6))
                                .when(!enabled, |view| view.opacity(0.6))
                                .child(icon(provider_icon(&id)).size(px(16.))),
                        )
                        .child(summary.when(!enabled, |view| view.opacity(0.6)))
                        .child(toggle)
                        .child(crate::shell::icon_button(
                            ("configure-provider", index),
                            if open { "alt-arrow-up" } else { "alt-arrow-down" },
                            "Configure provider",
                        ).tab_index(if can_expand { 0 } else { -1 }).when(!can_expand, |view| view.opacity(0.35)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !can_expand { return; }
                            this.provider_expanded =
                                if this.provider_expanded.as_deref() == Some(&expand) {
                                    None
                                } else {
                                    Some(expand.clone())
                                };
                            if !this.provider_account_rows.contains_key(&expand) {
                                let rows=cx.new(crate::provider_accounts::AccountRows::new);
                                let provider=expand.clone();
                                let events=cx.subscribe(&rows,move|this,_,event:&crate::provider_accounts::Event,cx|this.provider_account_action(&provider,event,cx));
                                this.provider_account_events.insert(expand.clone(),events);
                                this.provider_account_rows.insert(expand.clone(),rows);
                            }
                            if !this.provider_codes.contains_key(&expand) {
                                let input = cx.new(|cx| {
                                    input::TextInput::new(
                                        "Paste authorization code or callback URL",
                                        window,
                                        cx,
                                    )
                                });
                                let provider=expand.clone();
                                let events=cx.subscribe(&input,move|this,_,_:&input::Submitted,cx|this.complete_provider_login(&provider,cx));
                                this.provider_code_events.insert(expand.clone(),events);
                                this.provider_codes.insert(expand.clone(), input);
                            }
                            cx.notify();
                        }));
                    let mut card = column()
                        .when(index > 0, |view| {
                            view.border_t_1().border_color(rgb(BORDER))
                        })
                        .child(header);
                    if open {
                        let mut details = column().gap(px(16.)).px(px(12.)).pt(px(4.)).pb(px(16.));
                        if !installed && provider["canInstall"] != true {
                            let command = serde_json::from_value::<zuse_protocol::HarnessId>(json!(id)).ok().and_then(zuse_protocol::provider_setup::manual_command);
                            details = details.child(s::details("Install agent", command.unwrap_or("Install this agent on the execution host, then refresh its status.")));
                            if let Some(command) = command {
                                details = details.child(s::button(("copy-install", index), "Copy command").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(command.to_owned()))));
                            }
                        } else if !installed {
                            let install = id.clone();
                            details = details.child(
                                row()
                                    .gap(px(12.))
                                    .child(s::details(
                                        "Install agent",
                                        format!("Install {name} on the execution host."),
                                    ))
                                    .child(
                                        s::button(
                                            ("install-provider", index),
                                            if busy { "Installing…" } else { "Install" },
                                        )
                                        .when(busy, |button| button.opacity(0.5))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.provider_action(
                                                    install.clone(),
                                                    methods::INSTALL_HARNESS,
                                                    json!({"harness":install}),
                                                );
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            );
                        } else if let Ok(harness)=zuse_protocol::HarnessId::try_from(id.clone()) {
                            use crate::provider_signin::{self as signin,Data,Phase};
                            let login=self.provider_logins.get(&id);let cancelled=login.is_some_and(|login|login["cancelled"]==true);
                            let accounts=self.accounts["accounts"].as_array().into_iter().flatten().filter(|a|a["harness"]==id).filter_map(|a|serde_json::from_value::<zuse_protocol::AgentAccount>(a.clone()).ok()).collect::<Vec<_>>();
                            let empty=accounts.is_empty();
                            let login_pending=!cancelled&&login.is_some_and(|login|matches!(login["status"].as_str(),Some("starting"|"pending")));
                            if let Some(rows)=self.provider_account_rows.get(&id){
                                rows.update(cx,|rows,cx|rows.sync(crate::provider_accounts::Data{scope:self.connection_epoch.to_string(),accounts,blocked:busy||login_pending||!self.connected},cx));
                                if !empty {details=details.child(rows.clone());}
                            }
                            let phase=if cancelled{Phase::Idle}else{match login.and_then(|login|login["status"].as_str()){
                                Some("starting"|"pending")=>Phase::Waiting{message:login.and_then(|login|login["message"].as_str()).map(str::to_owned)},
                                Some("failed")=>Phase::Failed(login.and_then(|login|login["message"].as_str()).unwrap_or("Sign-in failed.").into()),
                                Some("done")=>Phase::Success,_=>Phase::Idle,
                            }};
                            let code=(!cancelled&&login.is_some_and(|login|login["mode"]=="paste-code"&&login["status"]=="pending")).then(||self.provider_codes.get(&id).cloned().map(IntoElement::into_any_element)).flatten();
                            let submitting=self.provider_actions.iter().any(|(request,provider)|provider==&id&&self.pending.get(request).is_some_and(|method|method==methods::COMPLETE_AGENT_LOGIN));
                            if empty||login.is_some(){
                                details=details.child(signin::render(Data{harness,phase,url:login.and_then(|login|login["url"].as_str()).filter(|url|signin::is_openable_login_url(url)).map(str::to_owned),submitting,error:login.and_then(|login|login["error"].as_str()).map(str::to_owned),blocked:busy},code,Self::inline_provider_action,cx));
                            }else if harness!=zuse_protocol::HarnessId::Antigravity {
                                let mut add=row().gap(px(8.)).flex_wrap();
                                for(index,option)in signin::login_options(harness).into_iter().enumerate(){
                                    let label=option.provider.map_or_else(||"Add account".to_owned(),|_|format!("Add {} account",option.label));
                                    add=add.child(s::button(("provider-add-account",index),"").aria_label(label.clone()).border_0().bg(transparent_black()).child(icon("plus").size(px(12.))).child(label)
                                        .on_click(cx.listener(move|this,_,window,cx|this.inline_provider_action(signin::Action{harness,kind:signin::Kind::Start(option.provider)},window,cx))));
                                }
                                details=details.child(add);
                            }
                        }
                        let options_open=self.provider_options.contains(&id);
                        let phase=self.provider_updates.iter().find(|status|String::from(status.harness)==id).map(|status|status.phase);
                        let options_id=id.clone();
                        details=details.child(column().gap(px(8.))
                            .child(crate::provider_options::header(("provider-options",index),options_open,phase,false)
                                .on_click(cx.listener(move|this,_,_,cx| {
                                    if !this.provider_options.insert(options_id.clone()){this.provider_options.remove(&options_id);}
                                    cx.notify();
                                })))
                            .when(options_open,|view|view.child(div().pl(px(30.)).child(self.provider_update_details(&id,index,cx)))));
                        card = card.child(details);
                    }
                    card
                });
        column().gap(px(16.)).child(s::frame().child(s::header_with_action("Agent providers","Enable the coding agents you use, verify their local setup, and control which models appear in Zuse.",Some(refresh.into_any_element())))
            .child(s::card().when_some(self.provider_error.clone(),|card,error|card.child(div().px_3().py_2().text_size(px(11.)).text_color(rgb(0xf87171)).child(error))).children(rows).when_some(self.provider_updates_error.clone(),|card,error|card.child(s::description_text(error).px_3().py_2())))).child(self.custom_acp_settings(cx)).into_any_element()
    }
}
