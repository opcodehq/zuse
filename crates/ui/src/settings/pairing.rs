//! Native Devices cards use the profile-owned browser and pairing authorities.
use super::access_widgets::{details, field};
use crate::localization::message as tr;
use crate::{
    composer::ComposerInput,
    settings::widgets,
    state::{AppState, EngineHandle},
    theme::Theme,
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use zuse_i18n::messages as msg;
use zuse_rpc::methods;

pub struct PairingCard {
    state: Entity<AppState>,
    engine: Option<EngineHandle>,
    status: Value,
    relay_status: Value,
    direct_status: Value,
    relay_origin: Entity<ComposerInput>,
    relay_credential: Entity<ComposerInput>,
    peers: Vec<zuse_remote::PeerMetadata>,
    invitation: Option<zuse_remote::PairedTarget>,
    browser_link: Option<String>,
    name: Entity<ComposerInput>,
    listen: Entity<ComposerInput>,
    origin: Entity<ComposerInput>,
    certificate: Entity<ComposerInput>,
    key: Entity<ComposerInput>,
    destination: Entity<ComposerInput>,
    error: Option<String>,
    busy: bool,
    advanced: bool,
    dialog: Option<Dialog>,
    dialog_focus: FocusHandle,
    dialog_root: FocusHandle,
    return_focus: Option<FocusHandle>,
    epoch: u64,
    load: Option<Task<()>>,
    action: Option<Task<()>>,
    _state: Subscription,
}
impl PairingCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let engine = state.read(cx).engine().cloned();
        let observe = cx.observe(&state, |this: &mut Self, _, cx| {
            let next = this.state.read(cx).engine().cloned();
            let same = match (&this.engine, &next) {
                (Some(old), Some(new)) => old.same_connection(new),
                (None, None) => true,
                _ => false,
            };
            if same {
                return;
            }
            this.engine = next;
            this.epoch = this.epoch.wrapping_add(1);
            this.load = None;
            this.action = None;
            this.invitation = None;
            this.dialog = None;
            this.peers.clear();
            this.status = Value::Null;
            this.relay_status = Value::Null;
            this.direct_status = Value::Null;
            this.relay_origin
                .update(cx, |input, cx| input.set_text("", cx));
            this.relay_credential
                .update(cx, |input, cx| input.set_text("", cx));
            this.busy = false;
            this.error = None;
            this.refresh(cx);
            cx.notify();
        });
        let name = cx.new(|cx| ComposerInput::new("Device name", cx));
        let listen = cx.new(|cx| ComposerInput::new("127.0.0.1:27656", cx));
        listen.update(cx, |input, cx| input.set_text("127.0.0.1:27656", cx));
        let origin = cx.new(|cx| ComposerInput::new("https://your-computer.example:27656", cx));
        let certificate = cx.new(|cx| ComposerInput::new("Certificate file path", cx));
        let key = cx.new(|cx| ComposerInput::new("Private key file path", cx));
        let destination = cx.new(|cx| ComposerInput::new("Paste a private pairing link", cx));
        let relay_origin = cx.new(|cx| ComposerInput::new("https://relay.example.com", cx));
        let relay_credential =
            cx.new(|cx| ComposerInput::new("Private host credential file on this computer", cx));
        let mut card = Self {
            relay_status: Value::Null,
            direct_status: Value::Null,
            relay_origin,
            relay_credential,
            state,
            engine,
            status: Value::Null,
            peers: vec![],
            invitation: None,
            browser_link: None,
            name,
            listen,
            origin,
            certificate,
            key,
            destination,
            error: None,
            busy: false,
            advanced: false,
            dialog: None,
            dialog_focus: cx.focus_handle().tab_stop(true),
            dialog_root: cx.focus_handle(),
            return_focus: None,
            epoch: 0,
            load: None,
            action: None,
            _state: observe,
        };
        card.refresh(cx);
        card
    }
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let epoch = self.epoch;
        self.load = Some(cx.spawn(async move |this, cx| {
            let (status, peers, relay, direct) = futures::join!(
                engine
                    .client()
                    .call(methods::BROWSER_ACCESS_STATUS, json!({})),
                engine.client().call(methods::LIST_PAIRINGS, json!({})),
                engine
                    .client()
                    .call(methods::RELAY_ACCESS_STATUS, json!({})),
                engine
                    .client()
                    .call(methods::DIRECT_ACCESS_STATUS, json!({}))
            );
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
                }
                // Older hosts have no direct access to report.
                if let Ok(status) = direct {
                    card.direct_status = status;
                }
                match relay {
                    Ok(status) => card.relay_status = status,
                    Err(error) => card.error = Some(error.to_string()),
                }
                match status {
                    Ok(status) => card.status = status,
                    Err(error) => card.error = Some(error.to_string()),
                }
                match peers.and_then(|peers| {
                    serde_json::from_value(peers).map_err(|_| {
                        zuse_rpc::RpcError::Failed("Invalid paired device list".into())
                    })
                }) {
                    Ok(peers) => card.peers = peers,
                    Err(error) => card.error = Some(error.to_string()),
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
        self.busy = true;
        self.error = None;
        let epoch = self.epoch;
        self.action=Some(cx.spawn(async move|this,cx|{
            let result=engine.client().call(method,params).await;
            let _=this.update(cx,|card,cx|{
                if card.epoch!=epoch{return;}
                card.busy=false;
                match result {
                    Ok(value)=>{
                        if method==methods::CREATE_PAIRING {
                            card.invitation=value["fragment"].as_str().and_then(|fragment|zuse_remote::Invitation::parse_fragment(fragment).ok())
                                .and_then(|invitation|value["origin"].as_str().and_then(|origin|zuse_remote::PairedTarget::for_route(origin,invitation).ok()));
                            card.browser_link=value["browserOrigin"].as_str().filter(|origin|value["origin"].as_str()!=Some(*origin)).zip(value["fragment"].as_str()).map(|(origin,fragment)|format!("{origin}/{fragment}"));
                            if card.invitation.is_none(){card.error=Some("The invitation was created, but its connection URL is unavailable. Start browser or relay access before sharing it.".into());}
                        }else if matches!(method, methods::REVOKE_PAIRING | methods::START_DIRECT_ACCESS | methods::STOP_DIRECT_ACCESS | methods::START_RELAY_ACCESS | methods::STOP_RELAY_ACCESS | methods::RESUME_RELAY_ACCESS | methods::SET_RELAY_AUTO_START){card.invitation=None;}
                        if method != methods::CREATE_PAIRING { card.dialog = None; }
                        card.refresh(cx);
                    },
                    Err(error)=>{card.error=Some(error.to_string());card.refresh(cx);},
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        let optional = |input: &Entity<ComposerInput>| {
            let text = input.read(cx).text().trim().to_owned();
            (!text.is_empty()).then_some(text)
        };
        let params = json!({"listen":self.listen.read(cx).text().trim(),"publicOrigin":optional(&self.origin),"tlsCert":optional(&self.certificate),"tlsKey":optional(&self.key)});
        self.request(methods::START_BROWSER_ACCESS, params, cx);
    }
    fn start_relay(&mut self, cx: &mut Context<Self>) {
        let origin = self.relay_origin.read(cx).text().trim().to_owned();
        let credential = self.relay_credential.read(cx).text().trim().to_owned();
        if self.relay_status["configured"] == true && origin.is_empty() && credential.is_empty() {
            self.request(methods::RESUME_RELAY_ACCESS, json!({}), cx);
        } else {
            self.request(
                methods::START_RELAY_ACCESS,
                json!({"origin":origin,"hostTokenFile":credential}),
                cx,
            );
        }
    }
    fn create(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            self.error = Some("Enter a name for the device you want to pair.".into());
            cx.notify();
            return;
        }
        self.request(methods::CREATE_PAIRING, json!({"label":name}), cx);
    }
    fn open_destination(&mut self, cx: &mut Context<Self>) {
        let target = zuse_remote::PairedTarget::parse_link(self.destination.read(cx).text().trim());
        match target {
            Ok(target) => {
                self.destination
                    .update(cx, |input, cx| input.set_text("", cx));
                crate::open_paired_window(target, cx);
            }
            Err(_) => {
                self.error =
                    Some("Enter a valid private pairing link from the other computer.".into())
            }
        }
        cx.notify();
    }
}
#[derive(Clone)]
enum Dialog {
    Network,
    Invitation,
    Computer,
}

pub(super) struct PairingParts {
    pub method: AnyElement,
    pub connect: AnyElement,
    pub devices: AnyElement,
    pub add_computer: AnyElement,
    pub advanced: Option<AnyElement>,
    pub dialog: Option<AnyElement>,
}

impl PairingCard {
    fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.return_focus = window.focused(cx);
        self.dialog = Some(dialog);
        self.error = None;
        window.focus(&self.dialog_focus, cx);
        cx.notify();
    }
    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.dialog = None;
        self.invitation = None;
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
            self.invitation = None;
            cx.notify();
        }
        true
    }
    pub(super) fn parts(&mut self, cx: &mut Context<Self>) -> PairingParts {
        use zuse_viewport::{remote_access_style as r, settings_style as s};
        let theme = Theme::of(cx).for_settings_surface();
        let running = self.status["running"] == true;
        let relay_running = self.relay_status["running"] == true;
        let managed = self.status["managed"] == true;
        let ready = running || relay_running;
        let busy = self.busy;
        let description = if running {
            format!(
                "Available on this network · {}",
                self.status["origin"].as_str().unwrap_or("Running")
            )
        } else {
            "Connect from another device on the same Wi-Fi.".into()
        };
        let method = r::method_row("wifi", "Local network", description, false)
            .child(
                r::method_button(
                    "start-browser-access",
                    if busy {
                        "Updating…"
                    } else if running {
                        "Turn off"
                    } else {
                        "Turn on"
                    },
                    cx,
                )
                .when(busy || running && !managed, |button| {
                    button.opacity(0.5).cursor_default()
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if busy || running && !managed {
                        return;
                    }
                    this.open_dialog(Dialog::Network, window, cx);
                })),
            )
            .into_any_element();
        let connect = r::frame(
            crate::localization::text("settings-connect_link_card_connect_another_device", cx),
            crate::localization::text("settings-connect_link_card_create_a_temporary_link_that_authorizes_another_zuse_app_or_browser_to", cx),
            None,
        )
        .child(
            s::card().child(
                r::connect_row(ready, cx)
                    .child(
                        r::small_button("native-create-invitation", crate::localization::text("settings-connect_link_card_create_link", cx))
                            .when(!ready || busy, |v| v.opacity(0.5).cursor_default())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if ready && !busy {
                                    this.open_dialog(Dialog::Invitation, window, cx);
                                }
                            })),
                    ),
            ),
        )
        .into_any_element();
        let devices = r::frame(
            crate::localization::text("chat-environment_summary_connected_devices", cx),
            "Manage devices that have connected to this computer.",
            Some(
                r::button(
                    "native-refresh-pairings",
                    crate::localization::text("settings-organizations_refresh", cx),
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                .into_any_element(),
            ),
        )
        .child(
            s::card()
                .when(self.peers.is_empty(), |v| {
                    v.child(r::empty("No other devices are connected yet."))
                })
                .children(self.peers.iter().enumerate().map(|(index, peer)| {
                    let id = peer.id.clone();
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
                            zuse_viewport::remote_access_style::glyph("smartphone")
                                .size(px(16.))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            r::details(
                                peer.label.clone(),
                                if peer.paired_at.is_some() {
                                    crate::localization::text(
                                        "settings-cloud_workspace_auth_ready",
                                        cx,
                                    )
                                } else if peer.expires_at.is_some_and(|expiry| {
                                    expiry <= chrono::Utc::now().timestamp().max(0) as u64
                                }) {
                                    ("Invitation expired").to_string()
                                } else {
                                    crate::localization::text(
                                        "settings-organizations_invitation_pending",
                                        cx,
                                    )
                                },
                            )
                            .flex_1(),
                        )
                        .child(
                            r::small_button(
                                ("native-revoke-pairing", index),
                                crate::localization::text("settings-organizations_revoke", cx),
                            )
                            .border_color(gpui::transparent_black())
                            .text_color(zuse_viewport::settings_style::destructive_text())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.request(methods::REVOKE_PAIRING, json!({"pairId":id}), cx);
                                    cx.notify();
                                },
                            )),
                        )
                })),
        )
        .into_any_element();
        let add_computer = r::add_computer_button("native-open-paired-workspace")
            .on_click(
                cx.listener(|this, _, window, cx| this.open_dialog(Dialog::Computer, window, cx)),
            )
            .into_any_element();
        let direct_running = self.direct_status["running"] == true;
        let direct_serve = self.direct_status["tailscaleServe"] == true;
        let direct_routes = zuse_viewport::remote_access_style::route_summary(&self.direct_status);
        let connected_through = self
            .engine
            .as_ref()
            .and_then(|e| e.link_status())
            .map(|link| {
                let latency = link
                    .rtt_ms
                    .map(|ms| format!(" · {ms} ms"))
                    .unwrap_or_default();
                format!("Connected through {}{latency}", link.route.kind.label())
            });
        let direct = r::frame("Direct connections","Let paired devices reach this computer over your local network or Tailscale. Zuse uses whichever route is fastest and stays end-to-end encrypted.",None)
            .child(s::card()
                .when_some(connected_through,|v,line|v.child(widgets::card_row(&theme,true).child(details(&theme,"This window",&line))))
                .child(widgets::card_row(&theme,true).child(details(&theme,"Local network and Tailscale",if direct_running {direct_routes.as_str()}else{"Off"}))
                    .child(widgets::toggle_switch(&theme,direct_running,"native-direct-access").id("native-direct-access").role(Role::Switch).aria_label("Allow local network and Tailscale connections").aria_toggled(if direct_running {gpui::Toggled::True}else{gpui::Toggled::False}).tab_index(0)
                        .on_click(cx.listener(move|this,_,_,cx|if direct_running {this.request(methods::STOP_DIRECT_ACCESS,json!({}),cx)}else{this.request(methods::START_DIRECT_ACCESS,json!({"tailscaleServe":direct_serve}),cx)}))))
                .when(direct_running&&self.direct_status["tailscaleAvailable"]==true,|v|v.child(widgets::card_row(&theme,false).child(details(&theme,"Tailscale HTTPS","Also publish on your tailnet with a trusted certificate, so browsers can use it."))
                    .child(widgets::toggle_switch(&theme,direct_serve,"native-tailscale-serve").id("native-tailscale-serve").role(Role::Switch).aria_label("Publish over Tailscale HTTPS").aria_toggled(if direct_serve {gpui::Toggled::True}else{gpui::Toggled::False}).tab_index(0)
                        .on_click(cx.listener(move|this,_,_,cx|this.request(methods::START_DIRECT_ACCESS,json!({"tailscaleServe":!direct_serve}),cx))))))
                .when_some(self.direct_status["error"].as_str(),|v,error|v.child(widgets::error_strip(&theme,error.to_owned()))));
        let advanced = Some(div().flex().flex_col().gap(px(10.))
            .child(direct)
            .child(div().flex().justify_end().child(r::button("advanced-connection-settings",if self.advanced {"Hide advanced"}else{"Advanced connection settings"})
                .on_click(cx.listener(|this,_,_,cx|{this.advanced=!this.advanced;cx.notify();}))))
            .when(self.advanced,|view| view.child(r::frame("Relay access","Configure an outbound relay for connections across networks.",None)
                .child(s::card()
                    .child(widgets::card_row(&theme,true).child(details(&theme,&tr(cx, msg::native::relay),if relay_running {self.relay_status["origin"].as_str().unwrap_or("Running")}else{"Stopped"}))
                        .child(r::button("native-relay-access",if relay_running {"Stop relay"}else{"Start relay"})
                            .on_click(cx.listener(move|this,_,_,cx|if relay_running {this.request(methods::STOP_RELAY_ACCESS,json!({}),cx)}else{this.start_relay(cx)}))))
                    .child(widgets::card_row(&theme,false).child(details(&theme,&tr(cx, msg::native::start_when_zuse_opens),&tr(cx, msg::native::start_relay_once_to_save_its_configuration)))
                        .child(widgets::toggle_switch(&theme,self.relay_status["autoStart"]==true,"native-relay-auto-start").id("native-relay-auto-start").role(Role::Switch).aria_label(tr(cx, msg::native::start_relay_when_zuse_opens)).aria_toggled(if self.relay_status["autoStart"]==true {gpui::Toggled::True}else{gpui::Toggled::False}).tab_index(0)
                            .on_click(cx.listener(|this,_,_,cx|this.request(methods::SET_RELAY_AUTO_START,json!({"enabled":this.relay_status["autoStart"]!=true}),cx)))))
                    .when(!relay_running,|v|v.child(widgets::card_row(&theme,false).child(details(&theme,&tr(cx, msg::native::relay_url),&tr(cx, msg::native::leave_both_fields_blank_to_resume_the_saved_relay))).child(field(&theme,self.relay_origin.clone())))
                        .child(widgets::card_row(&theme,false).child(details(&theme,&tr(cx, msg::native::host_credential),&tr(cx, msg::native::private_token_file_on_this_computer))).child(field(&theme,self.relay_credential.clone()))))
                    .when_some(self.relay_status["error"].as_str(),|v,error|v.child(widgets::error_strip(&theme,error.to_owned()))))))
            .when(self.dialog.is_none(),|v|v.when_some(self.error.clone(),|v,error|v.child(widgets::error_strip(&theme,error))))
            .into_any_element());
        let dialog = self.dialog.clone().map(|dialog| {
            let (title,description)=match &dialog {
                Dialog::Network if running => ("Turn off local network access?","Devices using this connection will disconnect. You can turn it on again at any time."),
                Dialog::Network => ("Turn on local network access?","Choose the address and optional HTTPS configuration for this computer."),
                Dialog::Invitation => ("Connect another device","Open the private link on the device you want to connect. Unused invitations expire in ten minutes."),
                Dialog::Computer => ("Add computer","Paste a connection link from the other computer to open its workspace."),
            };
            let mut body=r::dialog_body();
            match &dialog {
                Dialog::Network if !running => {
                    body=body.child(r::details("Listen address","Use localhost for this computer or a LAN address with trusted HTTPS.")).child(field(&theme,self.listen.clone()))
                        .child(r::details("Public URL","HTTPS URL used by your other devices. Optional for localhost.")).child(field(&theme,self.origin.clone()))
                        .child(r::details("HTTPS certificate and key","File paths on this computer. Leave blank for a local connection."))
                        .child(field(&theme,self.certificate.clone())).child(field(&theme,self.key.clone()));
                }
                Dialog::Invitation => {
                    body=body.child(r::details("Device name","A name you recognize in Connected devices.")).child(field(&theme,self.name.clone()))
                        .child(r::button("native-generate-invitation",if busy {crate::localization::text("projects-github_creating", cx)}else{crate::localization::text("settings-connect_link_card_create_link", cx)})
                            .on_click(cx.listener(|this,_,_,cx|this.create(cx))));
                    if let Some(target)=self.invitation.clone() {
                        let copy=target.clone();
                        body=body.child(r::details("Private connection link","Share this link only with the device you want to connect."))
                            .child(div().flex().gap(px(8.)).child(r::button("native-copy-invitation",crate::localization::text("settings-connect_link_card_copy_link", cx)).on_click(move|_,_,cx|if let Ok(link)=copy.export_link(){cx.write_to_clipboard(ClipboardItem::new_string(link));}))
                                .child(r::button("native-open-invitation",crate::localization::text("chat-markdown_body_open_link", cx)).on_click(move|_,_,cx|if let Ok(link)=target.export_link(){cx.open_url(&link);} )));
                        if let Some(link)=self.browser_link.clone() {
                            body=body.child(r::details("Browser link","Open this link in a web browser. It may warn about this computer's private certificate the first time."))
                                .child(r::button("native-copy-browser-invitation","Copy browser link").on_click(move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))));
                        }
                    }
                }
                Dialog::Computer => { body=body.child(field(&theme,self.destination.clone())); }
                _ => {}
            }
            body=body.when_some(self.error.clone(),|v,error|v.child(widgets::error_strip(&theme,error)));
            let mut footer=r::dialog_footer().child(r::button("close-remote-dialog",if matches!(dialog,Dialog::Invitation){crate::localization::text("settings-cloud_api_keys_done", cx)}else{crate::localization::text("settings-organizations_cancel", cx)})
                .track_focus(&self.dialog_focus).on_click(cx.listener(|this,_,window,cx|this.close_dialog(window,cx))));
            if !matches!(dialog,Dialog::Invitation) {
                let caption=if busy {"Updating…"}else{match dialog {Dialog::Network if running=>"Turn off",Dialog::Network=>"Turn on",Dialog::Computer=>"Connect",_=>"Revoke access"}};
                footer=footer.child(r::button("confirm-remote-dialog",caption).when(busy,|v|v.opacity(0.5))
                    .on_click(cx.listener(move|this,_,_,cx| {
                        if this.busy {return;}
                        match &dialog {
                            Dialog::Network if running=>this.request(methods::STOP_BROWSER_ACCESS,json!({}),cx),
                            Dialog::Network=>this.start(cx),
                            Dialog::Computer=>this.open_destination(cx),
                            _=>{}
                        }
                    })));
            }
            r::Modal(r::dialog_panel("native-remote-dialog",title,description).track_focus(&self.dialog_root)
                .on_key_down(cx.listener(|this,event:&KeyDownEvent,window,cx|if event.keystroke.key=="escape"{this.close_dialog(window,cx);cx.stop_propagation();}))
                .child(body).child(footer).into_any_element()).into_any_element()
        });
        PairingParts {
            method,
            connect,
            devices,
            add_computer,
            advanced,
            dialog,
        }
    }
}
