//! Native Devices cards use the profile-owned browser and pairing authorities.
use crate::{
    composer::ComposerInput,
    settings::widgets,
    state::{AppState, EngineHandle},
    theme::Theme,
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use zuse_rpc::methods;

pub struct PairingCard {
    state: Entity<AppState>,
    engine: Option<EngineHandle>,
    status: Value,
    relay_status: Value,
    relay_origin: Entity<ComposerInput>,
    relay_credential: Entity<ComposerInput>,
    peers: Vec<zuse_remote::PeerMetadata>,
    invitation: Option<zuse_remote::PairedTarget>,
    name: Entity<ComposerInput>,
    listen: Entity<ComposerInput>,
    origin: Entity<ComposerInput>,
    certificate: Entity<ComposerInput>,
    key: Entity<ComposerInput>,
    destination: Entity<ComposerInput>,
    error: Option<String>,
    busy: bool,
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
            this.peers.clear();
            this.status = Value::Null;
            this.relay_status = Value::Null;
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
            relay_origin,
            relay_credential,
            state,
            engine,
            status: Value::Null,
            peers: vec![],
            invitation: None,
            name,
            listen,
            origin,
            certificate,
            key,
            destination,
            error: None,
            busy: false,
            epoch: 0,
            load: None,
            action: None,
            _state: observe,
        };
        card.refresh(cx);
        card
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        let epoch = self.epoch;
        self.load = Some(cx.spawn(async move |this, cx| {
            let (status, peers, relay) = futures::join!(
                engine
                    .client()
                    .call(methods::BROWSER_ACCESS_STATUS, json!({})),
                engine.client().call(methods::LIST_PAIRINGS, json!({})),
                engine
                    .client()
                    .call(methods::RELAY_ACCESS_STATUS, json!({}))
            );
            let _ = this.update(cx, |card, cx| {
                if card.epoch != epoch {
                    return;
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
                                .and_then(|invitation|value["origin"].as_str().and_then(|origin|zuse_remote::PairedTarget::new(origin,invitation).ok()));
                            if card.invitation.is_none(){card.error=Some("The invitation was created, but its connection URL is unavailable. Start browser or relay access before sharing it.".into());}
                        }else if matches!(method, methods::REVOKE_PAIRING | methods::START_RELAY_ACCESS | methods::STOP_RELAY_ACCESS | methods::RESUME_RELAY_ACCESS | methods::SET_RELAY_AUTO_START){card.invitation=None;}
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
fn header(theme: &Theme, title: &str, description: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(10.))
        .py(px(8.))
        .child(widgets::row_title(theme, title.to_owned()))
        .child(
            div()
                .text_size(crate::typography::ui_rems(11.))
                .line_height(crate::typography::ui_rems(16.))
                .text_color(theme.text_muted)
                .child(description.to_owned()),
        )
}
fn frame(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .p(px(4.))
        .rounded(px(zuse_viewport::settings_style::FRAME_RADIUS))
        .bg(theme.settings_frame_bg())
}
fn field(theme: &Theme, input: Entity<ComposerInput>) -> Div {
    div()
        .flex_1()
        .min_w(px(180.))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.input_glass_bg())
        .child(input)
}
fn details(theme: &Theme, title: &str, description: &str) -> Div {
    div()
        .flex_1()
        .min_w(px(160.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(widgets::row_title(theme, title.to_owned()))
        .child(
            div()
                .text_size(crate::typography::ui_rems(11.))
                .text_color(theme.text_muted)
                .child(description.to_owned()),
        )
}
impl Render for PairingCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let running = self.status["running"] == true;
        let relay_running = self.relay_status["running"] == true;
        let auto_start = self.relay_status["autoStart"] == true;
        let can_invite = running || relay_running;
        let managed = self.status["managed"] == true;
        let busy = self.busy;
        let invite = self.invitation.clone();
        div().flex().flex_col().gap(px(20.)).mt(px(24.))
            .child(frame(&theme).child(header(&theme,"Access methods","Choose how your other devices connect to this engine."))
                .child(widgets::section_card(&theme).mt(px(0.))
                    .child(widgets::card_row(&theme,true).child(details(&theme,"Browser access",if running {self.status["origin"].as_str().unwrap_or("Running")}else{"Stopped. Start it to connect a browser or another Zuse app."}))
                        .when(!running,|row|row.child(widgets::text_action(&theme,widgets::ActionTone::Filled,if busy{"Working…"}else{"Start"}).id("start-browser-access").on_click(cx.listener(|this,_,_,cx|this.start(cx)))))
                        .when(running&&managed,|row|row.child(widgets::text_action(&theme,widgets::ActionTone::Outlined,"Stop").id("stop-browser-access").on_click(cx.listener(|this,_,_,cx|this.request(methods::STOP_BROWSER_ACCESS,json!({}),cx)))))
                        .when(running&&!managed,|row|row.child(widgets::badge(&theme,"Server app"))))
                    .when(!running,|card|card
                        .child(widgets::card_row(&theme,false).child(details(&theme,"Listen address","Use localhost for this computer, or a LAN address with HTTPS." )).child(field(&theme,self.listen.clone())))
                        .child(widgets::card_row(&theme,false).child(details(&theme,"Public URL","Required for access from other devices. Use an HTTPS URL trusted by them.")).child(field(&theme,self.origin.clone())))
                        .child(widgets::card_row(&theme,false).child(details(&theme,"HTTPS certificate","Choose a certificate and matching private key for direct HTTPS."))
                            .child(div().flex_1().min_w(px(200.)).flex().flex_col().gap(px(8.)).child(field(&theme,self.certificate.clone())).child(field(&theme,self.key.clone())))))))
            .child(frame(&theme).child(header(&theme,"Relay access","Connect across networks through an outbound encrypted connection. Startup settings belong to this computer."))
                .child(widgets::section_card(&theme).mt(px(0.))
                    .child(widgets::card_row(&theme,true).child(details(&theme,"Relay",if relay_running {self.relay_status["origin"].as_str().unwrap_or("Running")}else{"Stopped"}))
                        .child(widgets::text_action(&theme,widgets::ActionTone::Outlined,if relay_running {"Stop relay"}else{"Start relay"}).id("native-relay-access")
                            .on_click(cx.listener(move|this,_,_,cx|{if relay_running {this.request(methods::STOP_RELAY_ACCESS,json!({}),cx);}else{this.start_relay(cx);}}))))
                    .child(widgets::card_row(&theme,false)
                        .child(details(&theme,"Start when Zuse opens","Start relay once to save its configuration. Stopping now does not change this preference."))
                        .child(widgets::toggle_switch(&theme,auto_start,"native-relay-auto-start")
                            .id("native-relay-auto-start")
                            .role(gpui::Role::Switch).tab_index(0)
                            .aria_label("Start when Zuse opens")
                            .aria_toggled(if auto_start { gpui::Toggled::True } else { gpui::Toggled::False })
                            .cursor_pointer()
                            .on_click(cx.listener(move|this,_,_,cx|this.request(methods::SET_RELAY_AUTO_START,json!({"enabled":!auto_start}),cx)))))
                    .when_some(self.relay_status["error"].as_str(),|card,error|card.child(widgets::card_row(&theme,false).child(details(&theme,"Relay needs attention",error))))
                    .when(!relay_running,|card|card
                        .child(widgets::card_row(&theme,false).child(details(&theme,"Relay URL","Use your relay’s HTTPS origin. Leave both fields blank to resume the saved relay.")).child(field(&theme,self.relay_origin.clone())))
                        .child(widgets::card_row(&theme,false).child(details(&theme,"Host credential","Choose the private token file issued by your relay operator on this engine’s computer.")).child(field(&theme,self.relay_credential.clone()))))
                    .when(relay_running,|card|card.child(widgets::card_row(&theme,false).child(details(&theme,"Transport",&format!("{} broker sockets · {} sessions. Refresh to update. Socket counts do not confirm application responsiveness.",self.relay_status["sockets"].as_u64().unwrap_or(0),self.relay_status["sessions"].as_u64().unwrap_or(0))))))))
            .child(frame(&theme).child(header(&theme,"Connect a device","Invitations grant access to this engine’s agents, files, terminals and settings."))
                .child(widgets::section_card(&theme).mt(px(0.))
                    .child(widgets::card_row(&theme,true).child(field(&theme,self.name.clone()))
                        .child(widgets::text_action(&theme,widgets::ActionTone::Filled,if busy{"Working…"}else{"Create invitation"}).id("native-create-invitation")
                            .when(!can_invite||busy,|button|button.opacity(0.45))
                            .on_click(cx.listener(move|this,_,_,cx|{if can_invite&&!busy{this.create(cx);}}))))
                    .when_some(invite,|card,target|{
                        let copy=target.clone();
                        card.child(widgets::card_row(&theme,false).child(details(&theme,"Private invitation","Share only with someone you trust. Unused invitations expire in ten minutes."))
                            .child(widgets::text_action(&theme,widgets::ActionTone::Filled,"Copy invitation").id("native-copy-invitation").on_click(move|_,_,cx|{if let Ok(link)=copy.export_link(){cx.write_to_clipboard(ClipboardItem::new_string(link));}}))
                            .child(widgets::text_action(&theme,widgets::ActionTone::Outlined,"Open in browser").id("native-open-invitation").on_click(move|_,_,cx|{if let Ok(link)=target.export_link(){cx.open_url(&link);}})))
                    })))
            .child(frame(&theme).child(header(&theme,"Connected devices","Revoke access when a device no longer needs this workspace."))
                .child(widgets::section_card(&theme).mt(px(0.))
                    .when(self.peers.is_empty(),|card|card.child(widgets::card_row(&theme,true).child(details(&theme,"No paired devices","Create an invitation to connect your first device."))))
                    .children(self.peers.iter().enumerate().map(|(index,peer)|{
                        let id=peer.id.clone();
                        widgets::card_row(&theme,index==0).child(details(&theme,&peer.label,if peer.paired_at.is_some(){"Paired"}else if peer.expires_at.is_some_and(|expiry|expiry<=chrono::Utc::now().timestamp().max(0)as u64){"Invitation expired"}else{"Invitation pending"}))
                            .child(widgets::text_action(&theme,widgets::ActionTone::Quiet,"Revoke").id(("native-revoke-pairing",index))
                                .on_click(cx.listener(move|this,_,_,cx|this.request(methods::REVOKE_PAIRING,json!({"pairId":id}),cx))))
                    }))))
            .child(frame(&theme).child(header(&theme,"Use another computer","Open a paired workspace in a separate Zuse window. Work on this computer continues."))
                .child(widgets::section_card(&theme).mt(px(0.)).child(widgets::card_row(&theme,true).child(field(&theme,self.destination.clone()))
                    .child(widgets::text_action(&theme,widgets::ActionTone::Filled,"Open workspace").id("native-open-paired-workspace").on_click(cx.listener(|this,_,_,cx|this.open_destination(cx)))))))
            .when_some(self.error.clone(),|view,error|view.child(widgets::error_strip(&theme,error)))
            .child(widgets::text_action(&theme,widgets::ActionTone::Quiet,"Refresh device access").id("native-refresh-pairings").on_click(cx.listener(|this,_,_,cx|this.refresh(cx))))
    }
}
