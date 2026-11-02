//! Pairing capabilities stay in this transient view, outside UI preferences.
use super::*;
use crate::settings_style as s;
use crate::shell::{column, row};

impl WorkspaceView {
    pub(super) fn refresh_pairings(&mut self) {
        self.refresh_relay();
        self.refresh_network_access();
        if !self.connected || self.pairing_fetch.is_some() {
            return;
        }
        self.pairing_fetch = Some(self.call(methods::LIST_PAIRINGS, json!({})));
    }
    fn pairing_action(&mut self, method: &'static str, params: Value) {
        if !self.connected || self.pairing_action.is_some() {
            return;
        }
        self.remote_ui.error = None;
        self.pairing_action = Some(self.call(method, params));
    }
    pub(super) fn receive_pairing(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.receive_network_access(id, frame, cx) {
            return true;
        }
        if self.relay_fetch == Some(id) || self.relay_action == Some(id) {
            let action = self.relay_action == Some(id);
            if let Some(error) = frame["err"].as_str() {
                self.error = Some(error.into());
            }
            if let Some(value) = frame.get("ok") {
                self.relay_status = value.clone();
            }
            self.pending.remove(&id);
            if action {
                self.relay_action = None;
                self.pairing_fragment = None;
                self.refresh_relay();
            } else {
                self.relay_fetch = None;
            }
            cx.notify();
            return true;
        }
        let method = self.pending.get(&id).cloned().unwrap_or_default();
        let fetch = self.pairing_fetch == Some(id);
        let action = self.pairing_action == Some(id);
        if !fetch && !action {
            return false;
        }
        if let Some(error) = frame["err"].as_str() {
            self.remote_ui.error = Some(error.into());
        }
        if let Some(value) = frame.get("ok") {
            if fetch {
                self.pairings = value.clone();
            }
            if action && method == methods::REVOKE_PAIRING {
                self.remote_ui.dialog = None;
            }
            if action {
                self.pairing_fragment = value["fragment"].as_str().map(str::to_owned);
                if let Some(origin) = value["origin"]
                    .as_str()
                    .and_then(|origin| zuse_remote::normalized_origin(origin).ok())
                {
                    self.pairing_origin = origin;
                }
                if let Some(fetch) = self.pairing_fetch.take() {
                    self.cancel(fetch);
                }
                self.refresh_pairings();
            }
        }
        self.pending.remove(&id);
        if fetch {
            self.pairing_fetch = None;
        }
        if action {
            self.pairing_action = None;
        }
        cx.notify();
        true
    }
    fn refresh_relay(&mut self) {
        if self.connected && self.relay_fetch.is_none() && self.relay_action.is_none() {
            self.relay_fetch = Some(self.call(methods::RELAY_ACCESS_STATUS, json!({})));
        }
    }
    fn send_relay_action(&mut self, method: &'static str, params: Value) {
        if !self.connected || self.relay_action.is_some() {
            return;
        }
        if let Some(id) = self.relay_fetch.take() {
            self.cancel(id);
        }
        self.relay_action = Some(self.call(method, params));
    }
    fn change_relay(&mut self, cx: &mut Context<Self>) {
        let origin = self.relay_origin.read(cx).value(cx).trim().to_owned();
        let credential = self.relay_credential.read(cx).value(cx).trim().to_owned();
        let (method, params) = if self.relay_status["running"] == true {
            (methods::STOP_RELAY_ACCESS, json!({}))
        } else if self.relay_status["configured"] == true
            && origin.is_empty()
            && credential.is_empty()
        {
            (methods::RESUME_RELAY_ACCESS, json!({}))
        } else {
            (
                methods::START_RELAY_ACCESS,
                json!({"origin":origin,"hostTokenFile":credential}),
            )
        };
        self.send_relay_action(method, params);
    }
}

#[derive(Clone)]
pub(super) enum RemoteDialog {
    Setup,
    Unlink,
    Invitation,
    Network,
    Computer,
}
pub(super) struct RemoteUi {
    pub(super) dialog: Option<RemoteDialog>,
    focus: FocusHandle,
    root: FocusHandle,
    return_focus: Option<FocusHandle>,
    destination: Entity<input::TextInput>,
    listen: Entity<input::TextInput>,
    origin: Entity<input::TextInput>,
    certificate: Entity<input::TextInput>,
    key: Entity<input::TextInput>,
    network: Value,
    read: Option<u64>,
    action: Option<u64>,
    confirmed: bool,
    error: Option<String>,
    advanced: bool,
}
impl RemoteUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<WorkspaceView>) -> Self {
        let listen = cx.new(|cx| input::TextInput::new("127.0.0.1:27656", window, cx));
        listen.update(cx, |input, cx| {
            input.set_value("127.0.0.1:27656".into(), cx)
        });
        Self {
            dialog: None,
            focus: cx.focus_handle(),
            root: cx.focus_handle(),
            return_focus: None,
            destination: cx
                .new(|cx| input::TextInput::new("Paste a private connection link", window, cx)),
            listen,
            origin: cx
                .new(|cx| input::TextInput::new("https://your-computer.example:27656", window, cx)),
            certificate: cx.new(|cx| input::TextInput::new("Certificate file path", window, cx)),
            key: cx.new(|cx| input::TextInput::new("Private key file path", window, cx)),
            network: Value::Null,
            read: None,
            action: None,
            confirmed: false,
            error: None,
            advanced: false,
        }
    }
    pub(super) fn interrupted(&mut self) {
        if self.action.is_some() {
            self.error=Some("The network change may have completed. Refresh the saved state after reconnecting before trying again.".into());
        }
        self.read = None;
        self.action = None;
        self.confirmed = false;
        self.dialog = None;
    }
}
impl WorkspaceView {
    pub(super) fn open_remote_dialog(
        &mut self,
        dialog: RemoteDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.remote_busy() {
            return;
        }
        self.remote_ui.dialog = Some(dialog.clone());
        self.remote_ui.error = None;
        self.remote_ui.return_focus = window.focused(cx);
        match dialog {
            RemoteDialog::Invitation => window.focus(&self.pairing_name.focus_handle(cx), cx),
            RemoteDialog::Computer => {
                window.focus(&self.remote_ui.destination.focus_handle(cx), cx)
            }
            _ => window.focus(&self.remote_ui.focus, cx),
        }
        cx.notify();
    }
    fn remote_busy(&self) -> bool {
        self.pairing_action.is_some()
            || self.remote_ui.action.is_some()
            || self.managed_access_busy()
    }
    pub(super) fn close_remote_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote_busy() {
            return;
        }
        self.remote_ui.dialog = None;
        self.pairing_fragment = None;
        if let Some(focus) = self.remote_ui.return_focus.take() {
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        cx.notify();
    }
    fn refresh_network_access(&mut self) {
        if self.connected && self.remote_ui.read.is_none() && self.remote_ui.action.is_none() {
            self.remote_ui.read = Some(self.call(methods::BROWSER_ACCESS_STATUS, json!({})));
        }
    }
    fn change_network_access(&mut self, cx: &mut Context<Self>) {
        if !self.connected || self.remote_ui.action.is_some() || !self.remote_ui.confirmed {
            return;
        }
        if let Some(id) = self.remote_ui.read.take() {
            self.cancel(id);
        }
        let optional = |field: &Entity<input::TextInput>| {
            let value = field.read(cx).value(cx);
            (!value.trim().is_empty()).then(|| value.trim().to_owned())
        };
        let (method, params) = if self.remote_ui.network["running"] == true {
            (methods::STOP_BROWSER_ACCESS, json!({}))
        } else {
            (
                methods::START_BROWSER_ACCESS,
                json!({"listen":self.remote_ui.listen.read(cx).value(cx).trim(),"publicOrigin":optional(&self.remote_ui.origin),"tlsCert":optional(&self.remote_ui.certificate),"tlsKey":optional(&self.remote_ui.key)}),
            )
        };
        self.remote_ui.confirmed = false;
        self.remote_ui.error = None;
        self.remote_ui.action = Some(self.call(method, params));
    }
    fn receive_network_access(&mut self, id: u64, frame: &Value, cx: &mut Context<Self>) -> bool {
        let action = self.remote_ui.action == Some(id);
        let read = self.remote_ui.read == Some(id);
        if !action && !read {
            return false;
        }
        if let Some(error) = frame["err"].as_str() {
            self.remote_ui.error = Some(error.into());
            self.remote_ui.confirmed = false;
        }
        if let Some(value) = frame.get("ok") {
            if read {
                self.remote_ui.network = value.clone();
                self.remote_ui.confirmed = true;
            }
            if action {
                self.remote_ui.dialog = None;
            }
        }
        self.pending.remove(&id);
        if read {
            self.remote_ui.read = None;
        }
        if action {
            self.remote_ui.action = None;
            self.refresh_network_access();
            self.refresh_pairings();
        }
        cx.notify();
        true
    }
    pub(super) fn pairing_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::remote_access_style as r;
        let running = self.remote_ui.network["running"] == true;
        let managed = self.remote_ui.network["managed"] == true;
        let ready = running || self.relay_status["running"] == true;
        let network_available = self.remote_ui.confirmed && self.connected;
        let busy = self.remote_busy();
        let description = if running {
            format!(
                "Available on this network · {}",
                self.remote_ui.network["origin"]
                    .as_str()
                    .unwrap_or("Running")
            )
        } else if !network_available {
            "Local network access is unavailable. Refresh to check again.".into()
        } else {
            "Connect from another device on the same Wi-Fi.".into()
        };
        let local = r::method_row("wifi", "Local network", description, false).child(
            r::method_button(
                "browser-network-access",
                if busy {
                    "Updating…"
                } else if running {
                    "Turn off"
                } else {
                    "Turn on"
                },
            )
            .when(!network_available || busy || running && !managed, |v| {
                v.opacity(0.5).cursor_default()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if network_available && !busy && (!running || managed) {
                    this.open_remote_dialog(RemoteDialog::Network, window, cx);
                }
            })),
        );
        let connect = r::frame(
            "Connect another device",
            "Create a temporary link that authorizes another Zuse app or browser to use this computer.",
            None,
        )
        .child(
            s::card().child(
                r::connect_row(ready)
                    .child(
                        r::small_button("create-pairing", "Create link")
                            .when(!ready || busy, |v| v.opacity(0.5).cursor_default())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if ready && !busy {
                                    this.open_remote_dialog(RemoteDialog::Invitation, window, cx);
                                }
                            })),
                    ),
            ),
        );
        let add = r::add_computer_button("add-remote-computer")
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_remote_dialog(RemoteDialog::Computer, window, cx)
            }))
            .into_any_element();
        let peers = self.pairings.as_array().cloned().unwrap_or_default();
        let devices = r::frame(
            "Connected devices",
            "Manage devices that have connected to this computer.",
            Some(
                r::button("refresh-pairings", "Refresh")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_pairings();
                        this.refresh_managed_access();
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
        )
        .child(
            s::card()
                .when(peers.is_empty(), |v| {
                    v.child(r::empty("No other devices are connected yet."))
                })
                .children(peers.into_iter().enumerate().map(|(index, peer)| {
                    let id = peer["id"].as_str().unwrap_or_default().to_owned();
                    let label = peer["label"].as_str().unwrap_or("Paired device").to_owned();
                    row()
                        .min_h(px(48.))
                        .px(px(12.))
                        .py(px(8.))
                        .gap(px(10.))
                        .when(index > 0, |v| {
                            v.border_t_1().border_color(s::card_border().opacity(0.4))
                        })
                        .child(crate::assets::icon("smartphone").size(px(16.)))
                        .child(
                            r::details(
                                label.clone(),
                                if peer["pairedAt"].is_number() {
                                    "Connected"
                                } else if peer["expiresAt"].as_u64().is_some_and(|expiry| {
                                    expiry <= chrono::Utc::now().timestamp().max(0) as u64
                                }) {
                                    "Invitation expired"
                                } else {
                                    "Invitation pending"
                                },
                            )
                            .flex_1(),
                        )
                        .child(
                            r::small_button(("revoke-pairing", index), "Revoke")
                                .border_color(gpui::transparent_black())
                                .text_color(s::destructive_text())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pairing_action(
                                        methods::REVOKE_PAIRING,
                                        json!({"pairId":id}),
                                    );
                                    cx.notify();
                                })),
                        )
                })),
        );
        column()
            .gap(px(10.))
            .p(px(12.))
            .child(
                r::frame(
                    "Remote access to this computer",
                    "Choose how other Zuse apps and browsers can reach this computer.",
                    None,
                )
                .child(s::card().child(self.managed_access_method(cx)).child(local)),
            )
            .child(connect)
            .child(self.managed_computers(add, cx))
            .child(devices)
            .children(self.managed_account_devices(cx))
            .child(
                row().justify_end().child(
                    r::button(
                        "advanced-connection-settings",
                        if self.remote_ui.advanced {
                            "Hide advanced"
                        } else {
                            "Advanced connection settings"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remote_ui.advanced = !this.remote_ui.advanced;
                        cx.notify();
                    })),
                ),
            )
            .when(self.remote_ui.advanced, |view| {
                view.child(self.relay_settings(cx))
            })
            .when(self.account_remote, |view| {
                view.child(
                    r::frame(
                        "Account connection",
                        "This tab runs on the selected computer.",
                        None,
                    )
                    .child(
                        s::card().child(
                            s::setting_row(true)
                                .child(s::details(
                                    "Return to controller",
                                    "Save local edits before returning to your controller.",
                                ))
                                .child(r::button("return-to-controller", "Return").on_click(
                                    cx.listener(|this, _, _, _| {
                                        let _ = this
                                            .requests
                                            .unbounded_send(json!({"ui":"returnToController"}));
                                    }),
                                )),
                        ),
                    ),
                )
            })
            .when(self.paired, |view| {
                view.child(
                    r::frame("This browser", "Manage the access saved in this tab.", None).child(
                        s::card().child(
                            s::setting_row(true)
                                .child(s::details(
                                    "Forget paired access",
                                    "This reloads the tab. Save local drafts first.",
                                ))
                                .child(
                                    r::button("forget-browser-pairing", "Forget pairing").on_click(
                                        cx.listener(|this, _, _, _| {
                                            let _ = this
                                                .requests
                                                .unbounded_send(json!({"ui":"forgetPairing"}));
                                        }),
                                    ),
                                ),
                        ),
                    ),
                )
            })
            .when(self.remote_ui.dialog.is_none(), |view| {
                view.when_some(
                    self.remote_ui
                        .error
                        .clone()
                        .or_else(|| self.managed_access_error()),
                    |v, error| v.child(s::description_text(error)),
                )
            })
            .children(self.remote_access_dialog(cx))
            .into_any_element()
    }
    fn relay_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::remote_access_style as r;
        let running = self.relay_status["running"] == true;
        let auto = self.relay_status["autoStart"] == true;
        r::frame(
            "Relay access",
            "Configure an outbound relay for connections across networks.",
            None,
        )
        .child(
            s::card()
                .child(
                    s::setting_row(true)
                        .child(s::details(
                            "Relay",
                            if running {
                                self.relay_status["origin"].as_str().unwrap_or("Running")
                            } else {
                                "Stopped"
                            },
                        ))
                        .child(
                            r::button(
                                "browser-relay-access",
                                if self.relay_action.is_some() {
                                    "Working…"
                                } else if running {
                                    "Stop relay"
                                } else {
                                    "Start relay"
                                },
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_relay(cx);
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    s::setting_row(false)
                        .child(s::details(
                            "Start when Zuse opens",
                            "Start relay once to save its configuration.",
                        ))
                        .child(
                            s::toggle("browser-relay-auto-start", auto, "Start when Zuse opens")
                                .tab_index(0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.send_relay_action(
                                        methods::SET_RELAY_AUTO_START,
                                        json!({"enabled":!auto}),
                                    );
                                    cx.notify();
                                })),
                        ),
                )
                .when(!running, |v| {
                    v.child(
                        s::setting_row(false)
                            .child(s::details(
                                "Relay URL",
                                "Leave both fields blank to resume the saved relay.",
                            ))
                            .child(s::field().child(self.relay_origin.clone())),
                    )
                    .child(
                        s::setting_row(false)
                            .child(s::details(
                                "Host credential",
                                "Private token file on this computer.",
                            ))
                            .child(s::field().child(self.relay_credential.clone())),
                    )
                })
                .when_some(self.relay_status["error"].as_str(), |v, error| {
                    v.child(s::description_text(error.to_owned()))
                }),
        )
        .into_any_element()
    }
    fn remote_access_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::remote_access_style as r;
        let dialog = self.remote_ui.dialog.clone()?;
        let busy = self.remote_busy();
        let running = self.remote_ui.network["running"] == true;
        let (title, description) = match &dialog {
            RemoteDialog::Setup => (
                "Set up Zuse Serve",
                "Link this computer to your Zuse account to access it from your other devices.",
            ),
            RemoteDialog::Unlink => (
                "Turn off Zuse Serve?",
                "This computer will no longer be available through your account. Local chats stay on this computer.",
            ),
            RemoteDialog::Invitation => (
                "Connect another device",
                "Open the private link on the device you want to connect. Unused invitations expire in ten minutes.",
            ),
            RemoteDialog::Computer => (
                "Add computer",
                "Paste a connection link from the other computer to open its workspace.",
            ),
            RemoteDialog::Network if running => (
                "Turn off local network access?",
                "Devices using this connection will disconnect. You can turn it on again at any time.",
            ),
            RemoteDialog::Network => (
                "Turn on local network access?",
                "Choose the address and optional HTTPS configuration for this computer.",
            ),
        };
        let mut body = r::dialog_body();
        match &dialog {
            RemoteDialog::Setup => body = body.child(self.managed_setup_form(cx)),
            RemoteDialog::Invitation => {
                body = body
                    .child(r::details(
                        "Device name",
                        "A name you recognize in Connected devices.",
                    ))
                    .child(s::field().child(self.pairing_name.clone()))
                    .child(
                        r::button(
                            "generate-pairing",
                            if busy { "Creating…" } else { "Create link" },
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let name = this.pairing_name.read(cx).value(cx);
                            if name.trim().is_empty() {
                                this.remote_ui.error =
                                    Some("Enter a name for the device you want to pair.".into());
                            } else {
                                this.remote_ui.error = None;
                                this.pairing_action(methods::CREATE_PAIRING, json!({"label":name}));
                            }
                            cx.notify();
                        })),
                    );
                if let Some(fragment) = &self.pairing_fragment {
                    let link = format!("{}/{}", self.pairing_origin, fragment);
                    let copy = link.clone();
                    body = body
                        .child(r::details(
                            "Private connection link",
                            "Share this link only with the device you want to connect.",
                        ))
                        .child(
                            row()
                                .gap(px(8.))
                                .child(r::button("copy-pairing-link", "Copy link").on_click(
                                    move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    },
                                ))
                                .child(
                                    r::button("open-pairing-link", "Open link")
                                        .on_click(move |_, _, cx| cx.open_url(&link)),
                                ),
                        );
                }
            }
            RemoteDialog::Computer => {
                body = body.child(s::field().child(self.remote_ui.destination.clone()))
            }
            RemoteDialog::Network if !running => {
                body = body
                    .child(r::details(
                        "Listen address",
                        "Use localhost for this computer or a LAN address with trusted HTTPS.",
                    ))
                    .child(s::field().child(self.remote_ui.listen.clone()))
                    .child(r::details(
                        "Public URL",
                        "HTTPS URL used by your other devices. Optional for localhost.",
                    ))
                    .child(s::field().child(self.remote_ui.origin.clone()))
                    .child(r::details(
                        "HTTPS certificate and key",
                        "File paths on this computer. Leave blank for a local connection.",
                    ))
                    .child(s::field().child(self.remote_ui.certificate.clone()))
                    .child(s::field().child(self.remote_ui.key.clone()))
            }
            _ => {}
        }
        body = body.when_some(
            self.remote_ui
                .error
                .clone()
                .or_else(|| self.managed_access_error()),
            |v, error| {
                v.child(
                    div()
                        .id("remote-access-error")
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .text_color(crate::palette::rgb(0xe5a0a0))
                        .child(error),
                )
            },
        );
        let mut footer = r::dialog_footer().child(
            r::button(
                "close-remote-dialog",
                if matches!(dialog, RemoteDialog::Invitation) {
                    "Done"
                } else {
                    "Cancel"
                },
            )
            .track_focus(&self.remote_ui.focus)
            .on_click(cx.listener(|this, _, window, cx| this.close_remote_dialog(window, cx))),
        );
        if !matches!(dialog, RemoteDialog::Invitation) {
            let enabled = !busy
                && self.connected
                && (!matches!(dialog, RemoteDialog::Setup)
                    || self.cloud_account["user"].is_object());
            let caption = if busy {
                "Updating…"
            } else {
                match &dialog {
                    RemoteDialog::Setup => "Link computer",
                    RemoteDialog::Network if !running => "Turn on",
                    RemoteDialog::Network | RemoteDialog::Unlink => "Turn off",
                    RemoteDialog::Computer => "Connect",
                    _ => "Revoke access",
                }
            };
            footer=footer.child(r::button("confirm-remote-dialog",caption).when(!enabled,|v|v.opacity(0.5).cursor_default())
                .on_click(cx.listener(move|this,_,_,cx|{
                    if !enabled||this.remote_busy(){return;}
                    match &dialog{
                        RemoteDialog::Setup=>{let label=this.managed_name.read(cx).value(cx).trim().to_owned();this.managed_action(methods::LINK_MANAGED_HOST,json!({"label":label}));},
                        RemoteDialog::Unlink=>this.managed_action(methods::UNLINK_MANAGED_HOST,json!({})),
                        RemoteDialog::Network=>this.change_network_access(cx),
                        RemoteDialog::Computer=>{
                            let link=this.remote_ui.destination.read(cx).value(cx);
                            match zuse_remote::PairedTarget::parse_link(link.trim()).and_then(|target|target.export_link()) {
                                Ok(link)=>{cx.open_url(&link);this.remote_ui.dialog=None;},
                                Err(_)=>this.remote_ui.error=Some("Enter a valid private connection link from the other computer.".into())
                            }
                        },
                        _=>{}
                    }
                    cx.notify();
                })));
        }
        Some(
            r::Modal(
                r::dialog_panel("remote-access-dialog", title, description)
                    .track_focus(&self.remote_ui.root)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.close_remote_dialog(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(body)
                    .child(footer)
                    .into_any_element(),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn relay_startup_actions_retire_old_reads_and_reconcile_uncertainty_without_replay(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut calls) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, _, cx| {
                view.connected = true;
                view.relay_status = json!({"running":false,"configured":true,"autoStart":false});
                view.refresh_relay();
                let stale = view.relay_fetch.unwrap();
                view.send_relay_action(methods::SET_RELAY_AUTO_START, json!({"enabled":true}));
                let action = view.relay_action.unwrap();
                view.send_relay_action(methods::SET_RELAY_AUTO_START, json!({"enabled":true}));
                assert!(view.relay_fetch.is_none());
                view.receive(
                    json!({"id":stale,"ok":{"configured":false,"autoStart":false}}),
                    cx,
                );
                assert_eq!(view.relay_status["configured"], true);
                view.receive(json!({"id":action,"err":"Receipt lost"}), cx);
                assert!(view.relay_action.is_none());
                let read = view.relay_fetch.unwrap();
                view.receive(
                    json!({"id":read,"ok":{"running":false,"configured":true,"autoStart":true}}),
                    cx,
                );
                assert_eq!(view.relay_status["autoStart"], true);
                view.change_relay(cx);
                let resume = view.relay_action.unwrap();
                view.receive(
                    json!({"id":resume,"ok":{"running":true,"configured":true,"autoStart":true}}),
                    cx,
                );
            })
            .unwrap();
        let mut writes = vec![];
        while let Ok(frame) = calls.try_recv() {
            if frame["method"] == methods::SET_RELAY_AUTO_START
                || frame["method"] == methods::RESUME_RELAY_ACCESS
                || frame["method"] == methods::START_RELAY_ACCESS
            {
                writes.push(frame);
            }
        }
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0]["method"], methods::SET_RELAY_AUTO_START);
        assert_eq!(writes[0]["params"]["enabled"], true);
        assert_eq!(writes[1]["method"], methods::RESUME_RELAY_ACCESS);
        assert_eq!(writes[1]["params"], json!({}));
    }
}
