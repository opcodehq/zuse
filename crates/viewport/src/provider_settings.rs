//! Provider setup follows Zuse's expandable rows and canonical engine actions.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BORDER, MUTED, column, provider_icon, row},
};
impl WorkspaceView {
    pub(super) fn provider_action(&mut self, provider: String, method: &'static str, params: Value) {
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
    pub(super) fn receive_provider_action(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(provider) = self.provider_actions.get(&id).cloned() else {
            return false;
        };
        let method = self.pending.get(&id).cloned().unwrap_or_default();
        if let Some(error) = frame["err"].as_str() {
            if provider.starts_with("acp-") {self.acp_provider_error(error.to_owned());}
            else {self.provider_error = Some(error.to_owned());}
            self.provider_actions.remove(&id);
            self.pending.remove(&id);
            cx.notify();
            return true;
        }
        let Some(value) = frame.get("ok") else {
            return true;
        };
        self.provider_actions.remove(&id);
        if matches!(
            method.as_str(),
            methods::SET_HARNESS_ENABLED | methods::INSTALL_HARNESS
        ) {
            return false;
        }
        match method.as_str() {
            methods::START_AGENT_LOGIN => {
                if let Some(url) = value["url"]
                    .as_str()
                    .filter(|url| url.starts_with("https://"))
                {
                    cx.open_url(url);
                }
                if let Some(input) = self.provider_codes.get(&provider) {
                    input.update(cx, |input, cx| input.set_value(String::new(), cx));
                }
                self.provider_logins.insert(provider, value.clone());
            }
            methods::POLL_AGENT_LOGIN => {
                let login = self.provider_logins.entry(provider.clone()).or_default();
                if let Some(url) = value["url"].as_str() {
                    login["url"] = json!(url);
                }
                if let Some(message) = value["message"].as_str() {
                    login["message"] = json!(message);
                }
                if value["status"] == "done" {
                    self.provider_logins.remove(&provider);
                    self.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":false}));
                    self.call(methods::LIST_HARNESSES, json!({}));
                }
                if value["status"] == "error" {
                    self.provider_error = value["message"].as_str().map(str::to_owned);
                }
            }
            methods::COMPLETE_AGENT_LOGIN | methods::CANCEL_AGENT_LOGIN => {
                self.provider_logins.remove(&provider);
                if let Some(input) = self.provider_codes.get(&provider) {
                    input.update(cx, |input, cx| input.set_value(String::new(), cx));
                }
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
                            if !this.provider_codes.contains_key(&expand) {
                                let input = cx.new(|cx| {
                                    input::TextInput::new(
                                        "Paste authorization code or callback URL",
                                        window,
                                        cx,
                                    )
                                });
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
                        } else {
                            let login = id.clone();
                            details = details.child(
                                row()
                                    .gap(px(12.))
                                    .child(s::details(
                                        "Account",
                                        account
                                            .and_then(|account| account["planLabel"].as_str())
                                            .unwrap_or("Sign in to use your agent subscription."),
                                    ))
                                    .child(
                                        s::button(
                                            ("sign-in-provider", index),
                                            if busy { "Working…" } else { "Sign in" },
                                        )
                                        .when(busy, |button| button.opacity(0.5))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.provider_action(
                                                    login.clone(),
                                                    methods::START_AGENT_LOGIN,
                                                    json!({"harness":login}),
                                                );
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            );
                        }
                        if let Some(login) = self.provider_logins.get(&id) {
                            let login_id = login["loginId"].as_str().unwrap_or_default().to_owned();
                            if let Some(url) = login["url"]
                                .as_str()
                                .filter(|url| url.starts_with("https://"))
                                .map(str::to_owned)
                            {
                                details = details.child(
                                    s::button(("open-provider-login", index), "Open sign-in page")
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                );
                            }
                            if let Some(input) = self.provider_codes.get(&id) {
                                let input = input.clone();
                                let field = input.clone();
                                let complete = id.clone();
                                let complete_id = login_id.clone();
                                details = details.child(
                                    row().gap(px(8.)).child(s::field().child(field)).child(
                                        s::button(
                                            ("complete-provider-login", index),
                                            "Complete sign-in",
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                let code = input.read(cx).value(cx);
                                                if code.trim().is_empty() {
                                                    return;
                                                }
                                                this.provider_action(
                                                    complete.clone(),
                                                    methods::COMPLETE_AGENT_LOGIN,
                                                    json!({"loginId":complete_id,"code":code}),
                                                );
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                                );
                            }
                            let poll = id.clone();
                            let poll_id = login_id.clone();
                            let cancel = id.clone();
                            details = details.child(
                                row()
                                    .gap(px(8.))
                                    .child(
                                        s::button(("check-provider-login", index), "Check sign-in")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.provider_action(
                                                    poll.clone(),
                                                    methods::POLL_AGENT_LOGIN,
                                                    json!({"loginId":poll_id}),
                                                );
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        s::button(("cancel-provider-login", index), "Cancel")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.provider_action(
                                                    cancel.clone(),
                                                    methods::CANCEL_AGENT_LOGIN,
                                                    json!({"loginId":login_id}),
                                                );
                                                cx.notify();
                                            })),
                                    ),
                            );
                            if let Some(message) = login["message"].as_str() {
                                details = details.child(s::description_text(message.to_owned()));
                            }
                        }
                        details = details.child(self.provider_update_details(&id,index,cx));
                        card = card.child(details);
                    }
                    card
                });
        column().gap(px(16.)).child(s::frame().child(s::header_with_action("Agent providers","Enable the coding agents you use, verify their local setup, and control which models appear in Zuse.",Some(refresh.into_any_element())))
            .child(s::card().when_some(self.provider_error.clone(),|card,error|card.child(div().px_3().py_2().text_size(px(11.)).text_color(rgb(0xf87171)).child(error))).children(rows).when_some(self.provider_updates_error.clone(),|card,error|card.child(s::description_text(error).px_3().py_2())))).child(self.custom_acp_settings(cx)).into_any_element()
    }
}
