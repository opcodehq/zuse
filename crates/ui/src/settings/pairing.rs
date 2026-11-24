//! Native Devices cards use the profile-owned browser and pairing authorities.
use super::access_widgets::{details, field};
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
    direct_status: Value,
    relay_origin: Entity<ComposerInput>,
    relay_credential: Entity<ComposerInput>,
    peers: Vec<zuse_remote::PeerMetadata>,
    invitation: Option<zuse_remote::PairedTarget>,
    browser_link: Option<String>,
    /// The pending device behind the code on screen, retired when a new code replaces it.
    code_peer: Option<String>,
    /// Refreshes the device list while the pairing dialog waits for a phone.
    poll: Option<Task<()>>,
    /// When the code on screen was made, for its countdown.
    code_created: Option<std::time::Instant>,
    /// 0 = phone or tablet (scan), 1 = another computer (link).
    pair_tab: usize,
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
            code_peer: None,
            poll: None,
            code_created: None,
            pair_tab: 0,
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
                            card.code_peer=value["peer"]["id"].as_str().map(str::to_owned);
                            card.code_created=Some(std::time::Instant::now());
                            card.invitation=value["fragment"].as_str().and_then(|fragment|zuse_remote::Invitation::parse_fragment(fragment).ok())
                                .and_then(|invitation|value["origin"].as_str().and_then(|origin|zuse_remote::PairedTarget::for_route(origin,invitation).ok()));
                            card.browser_link=value["browserOrigin"].as_str().filter(|origin|value["origin"].as_str()!=Some(*origin)).zip(value["fragment"].as_str()).map(|(origin,fragment)|format!("{origin}/{fragment}"));
                            if card.invitation.is_none(){card.error=Some("The code was created, but no address can reach this computer yet. Turn on access first.".into());}
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
    /// A fresh code for the next device. Devices are named after the day they joined; the
    /// list shows when each one connected, which is what tells them apart.
    fn create(&mut self, cx: &mut Context<Self>) {
        // An unused code leaves nothing behind when it is replaced.
        if let (Some(previous), Some(engine)) = (self.code_peer.take(), self.engine.clone()) {
            let unused = self
                .peers
                .iter()
                .any(|peer| peer.id == previous && peer.paired_at.is_none());
            if unused {
                cx.spawn(async move |_, _| {
                    let _ = engine
                        .client()
                        .call(methods::REVOKE_PAIRING, json!({"pairId": previous}))
                        .await;
                })
                .detach();
            }
        }
        let label = format!(
            "Device added {}",
            chrono::Local::now().format("%b %-d, %H:%M")
        );
        self.request(methods::CREATE_PAIRING, json!({"label":label}), cx);
    }
    fn open_destination(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = zuse_remote::PairedTarget::parse_link(self.destination.read(cx).text().trim());
        match target {
            Ok(target) => {
                self.destination
                    .update(cx, |input, cx| input.set_text("", cx));
                self.dialog = None;
                crate::open_remote_workspace(
                    crate::state::RemoteTarget::Paired(target),
                    window,
                    cx,
                );
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
    Computer,
    Pair,
}

pub(super) struct PairingParts {
    /// Whether this computer can be reached, the networks it offers and the Connect button.
    pub hero: AnyElement,
    /// Paired phones and computers, as rows for the Devices list.
    pub device_rows: Vec<AnyElement>,
    pub add_computer: AnyElement,
    /// The quiet link that shows or hides the advanced settings.
    pub advanced_toggle: AnyElement,
    /// Browser access and Tailscale HTTPS, when the advanced settings are open.
    pub advanced: Option<AnyElement>,
    pub dialog: Option<AnyElement>,
}

/// A small label above a card, with an optional action at its end, like the phone apps.
pub(super) fn section(
    theme: &Theme,
    title: &'static str,
    action: Option<AnyElement>,
    body: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(4.))
                .min_h(px(24.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(title),
                )
                .children(action),
        )
        .child(body)
}

/// A scannable code on white: dot modules, rounded finder corners and the Zuse mark in the
/// middle. High error correction keeps it readable with the centre covered.
fn qr_code(link: &str, side: f32) -> AnyElement {
    let Ok(code) = qrcode::QrCode::with_error_correction_level(link.as_bytes(), qrcode::EcLevel::H)
    else {
        return div().into_any_element();
    };
    let width = code.width();
    let dark: Vec<bool> = code
        .to_colors()
        .into_iter()
        .map(|color| color == qrcode::Color::Dark)
        .collect();
    const QUIET: usize = 1;
    let cells = (width + QUIET * 2) as f32;
    // The centre block cleared for the mark: about a fifth of the code, kept odd so it centres.
    let logo = ((width as f32 * 0.22).round() as usize) | 1;
    let logo_start = (width - logo) / 2;
    let in_logo = move |x: usize, y: usize| {
        (logo_start..logo_start + logo).contains(&x) && (logo_start..logo_start + logo).contains(&y)
    };
    let in_finder = move |x: usize, y: usize| {
        (x < 7 && y < 7) || (x >= width - 7 && y < 7) || (x < 7 && y >= width - 7)
    };
    let pad = 14.;
    let inner = side - pad * 2.;
    let logo_px = inner * logo as f32 / cells;
    div()
        .size(px(side))
        .flex_none()
        .rounded(px(20.))
        .bg(gpui::white())
        .p(px(pad))
        .relative()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let cell = f32::from(bounds.size.width) / cells;
                    let at = |x: f32, y: f32| {
                        bounds.origin
                            + point(px(cell * (x + QUIET as f32)), px(cell * (y + QUIET as f32)))
                    };
                    let ink = gpui::black();
                    for y in 0..width {
                        for x in 0..width {
                            if dark[y * width + x] && !in_finder(x, y) && !in_logo(x, y) {
                                let dot = cell * 0.82;
                                let inset = (cell - dot) / 2.;
                                window.paint_quad(
                                    gpui::fill(
                                        Bounds::new(
                                            at(x as f32, y as f32) + point(px(inset), px(inset)),
                                            size(px(dot), px(dot)),
                                        ),
                                        ink,
                                    )
                                    .corner_radii(px(dot / 2.)),
                                );
                            }
                        }
                    }
                    for (fx, fy) in [(0., 0.), ((width - 7) as f32, 0.), (0., (width - 7) as f32)] {
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(at(fx, fy), size(px(cell * 7.), px(cell * 7.))),
                                ink,
                            )
                            .corner_radii(px(cell * 2.)),
                        );
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(
                                    at(fx + 1., fy + 1.),
                                    size(px(cell * 5.), px(cell * 5.)),
                                ),
                                gpui::white(),
                            )
                            .corner_radii(px(cell * 1.4)),
                        );
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(
                                    at(fx + 2., fy + 2.),
                                    size(px(cell * 3.), px(cell * 3.)),
                                ),
                                ink,
                            )
                            .corner_radii(px(cell)),
                        );
                    }
                },
            )
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(logo_px * 0.92))
                        .rounded(px(logo_px * 0.26))
                        .bg(gpui::black())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            crate::icons::icon(crate::icons::ZUSE_LOGO)
                                .size(px(logo_px * 0.55))
                                .text_color(gpui::white()),
                        ),
                ),
        )
        .into_any_element()
}

/// A small filled dot; a soft ring around it when live.
fn status_dot(color: Hsla, live: bool) -> Div {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .when(live, |v| v.bg(color.opacity(0.18)))
        .child(div().size(px(8.)).rounded_full().bg(color))
}

/// The addresses a host offers, as short "Local network 192.168.1.4" labels.
fn route_labels(status: &Value) -> Vec<String> {
    status["routes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|route| {
            let kind = match route["kind"].as_str()? {
                "lan" => "Wi-Fi",
                "tailnet" => "Tailscale",
                _ => return None,
            };
            let host = route["origin"]
                .as_str()
                .and_then(|origin| url::Url::parse(origin).ok())
                .and_then(|url| url.host_str().map(str::to_owned));
            Some((kind, host))
        })
        // One entry per network: the first address, with a count of the rest.
        .fold(
            Vec::<(&str, Vec<String>)>::new(),
            |mut acc, (kind, host)| {
                match acc.iter_mut().find(|(k, _)| *k == kind) {
                    Some((_, hosts)) => hosts.extend(host),
                    None => acc.push((kind, host.into_iter().collect())),
                }
                acc
            },
        )
        .into_iter()
        .map(|(kind, hosts)| match hosts.as_slice() {
            [] => kind.to_owned(),
            [one] => format!("{kind} {one}"),
            [first, rest @ ..] => format!("{kind} {first} +{}", rest.len()),
        })
        .collect()
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
    /// Shows a fresh code and watches for the device that scans it.
    fn open_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_dialog(Dialog::Pair, window, cx);
        if self.invitation.is_none() || self.paired_code().is_some() {
            self.create(cx);
        }
        self.poll = Some(cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                tick += 1;
                let open = this
                    .update(cx, |card, cx| {
                        let open = matches!(card.dialog, Some(Dialog::Pair));
                        if open && card.paired_code().is_none() && tick % 2 == 0 {
                            card.refresh(cx);
                        }
                        cx.notify();
                        open
                    })
                    .unwrap_or(false);
                if !open {
                    break;
                }
            }
        }));
    }
    /// The device that used the code on screen, once it has.
    fn paired_code(&self) -> Option<&zuse_remote::PeerMetadata> {
        let id = self.code_peer.as_ref()?;
        self.peers
            .iter()
            .find(|peer| &peer.id == id && peer.paired_at.is_some())
    }
    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if matches!(self.dialog, Some(Dialog::Pair)) {
            self.poll = None;
            // A used code is spent; the next dialog makes a new one.
            if self.paired_code().is_some() {
                self.invitation = None;
            }
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
    /// `account` is the Zuse account row, listed among the ways to connect.
    pub(super) fn parts(
        &mut self,
        account: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> PairingParts {
        use zuse_viewport::{remote_access_style as r, settings_style as s};
        let theme = Theme::of(cx).for_settings_surface();
        let busy = self.busy;
        let browser_running = self.status["running"] == true;
        let managed = self.status["managed"] == true;
        let relay_running = self.relay_status["running"] == true;
        let direct_running = self.direct_status["running"] == true;
        let direct_serve = self.direct_status["tailscaleServe"] == true;
        let tailscale_available = self.direct_status["tailscaleAvailable"] == true;
        // Any listener another device can reach makes a pairing code useful.
        let reachable = direct_running || relay_running || browser_running;
        let routes = route_labels(&self.direct_status);

        // ---- Status, and every way another device can reach this computer ----
        let lan_on = direct_running && self.direct_status["lan"] != false;
        let tailnet_on = direct_running && self.direct_status["tailnet"] != false;
        let address = |kind: &str| -> Option<String> {
            self.direct_status["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|route| {
                    (route["kind"] == kind)
                        .then(|| {
                            route["origin"]
                                .as_str()
                                .and_then(|o| url::Url::parse(o).ok())
                        })
                        .flatten()
                        .and_then(|url| url.host_str().map(str::to_owned))
                })
        };
        let connected_through = self
            .engine
            .as_ref()
            .and_then(|e| e.link_status())
            .map(|link| {
                let latency = link
                    .rtt_ms
                    .map(|ms| format!(" · {ms} ms"))
                    .unwrap_or_default();
                format!(
                    "This window is connected through {}{latency}",
                    link.route.kind.label()
                )
            });
        let (headline, detail) = match (lan_on, tailnet_on, reachable) {
            (_, true, _) => ("Reachable at home and away", routes.join("  ·  ")),
            (true, false, _) => ("Reachable on your Wi-Fi", routes.join("  ·  ")),
            (false, false, true) => ("Reachable", "Through browser access.".to_owned()),
            _ => (
                "Only this computer can use Zuse",
                "Turn on a way to connect below. Zuse picks the fastest one on its own, and everything stays end-to-end encrypted.".to_owned(),
            ),
        };
        let dot = if reachable {
            theme.success
        } else {
            theme.text_faint
        };
        // Turning one network on or off restarts the listener with the new choice.
        let choose = move |lan: bool, tailnet: bool| -> (&'static str, Value) {
            if !lan && !tailnet {
                (methods::STOP_DIRECT_ACCESS, json!({}))
            } else {
                (
                    methods::START_DIRECT_ACCESS,
                    json!({"lan": lan, "tailnet": tailnet, "tailscaleServe": direct_serve && tailnet}),
                )
            }
        };
        let way = |icon: &'static str, title: &str, description: String, first: bool| {
            r::method_row(icon, title.to_owned(), description, first)
        };
        let switch = |key: &'static str, on: bool, label: &'static str, enabled: bool| {
            widgets::toggle_switch(&theme, on, key)
                .id(key)
                .role(Role::Switch)
                .aria_label(label)
                .aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .tab_index(0)
                .when(!enabled, |v| v.opacity(0.4).cursor_default())
        };
        let wifi = way(
            "wifi",
            "Wi-Fi",
            if lan_on {
                address("lan")
                    .map(|a| format!("On · {a}"))
                    .unwrap_or_else(|| "On · waiting for a network".into())
            } else {
                "Phones and computers on the same network.".into()
            },
            true,
        )
        .child(
            switch(
                "native-route-lan",
                lan_on,
                "Allow connections over Wi-Fi",
                !busy,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let (method, params) = choose(!lan_on, tailnet_on);
                this.request(method, params, cx)
            })),
        );
        let tailscale = way(
            "globe",
            "Tailscale",
            if tailnet_on {
                address("tailnet")
                    .map(|a| format!("On · {a}"))
                    .unwrap_or_else(|| "On · waiting for Tailscale".into())
            } else if tailscale_available {
                "Your tailnet, at home or away. Headscale works too.".into()
            } else {
                "Start Tailscale or Headscale on this computer to use it.".into()
            },
            false,
        )
        .child(
            switch(
                "native-route-tailnet",
                tailnet_on,
                "Allow connections over Tailscale",
                !busy && (tailscale_available || tailnet_on),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !tailscale_available && !tailnet_on {
                    return;
                }
                let (method, params) = choose(lan_on, !tailnet_on);
                this.request(method, params, cx)
            })),
        );
        let relay = way(
            "cloud",
            "Zuse Relay",
            "Reach it from any network without setting anything up.".into(),
            false,
        )
        .opacity(0.55)
        .child(
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(6.))
                .bg(s::frame_bg())
                .text_size(px(11.))
                .text_color(theme.text_muted)
                .child("Coming soon"),
        );
        let hero = s::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(12.))
                    .child(status_dot(dot, reachable))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(19.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(SharedString::from(headline)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(17.))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(detail)),
                            ),
                    )
                    .child(
                        r::button("native-open-pairing", "Connect device")
                            .when(!reachable || busy, |v| v.opacity(0.5).cursor_default())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if reachable && !this.busy {
                                    this.open_pair(window, cx)
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(s::card_border().opacity(0.4))
                    .child(wifi),
            )
            .child(tailscale)
            .children(account.map(|row| {
                div()
                    .border_t_1()
                    .border_color(s::card_border().opacity(0.4))
                    .child(row)
            }))
            .child(relay)
            .when_some(connected_through, |v, line| {
                v.child(
                    div()
                        .px(px(16.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(s::card_border().opacity(0.4))
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(line)),
                )
            })
            .when_some(self.direct_status["error"].as_str(), |v, error| {
                v.child(widgets::error_strip(&theme, error.to_owned()))
            })
            .into_any_element();

        // ---- Devices that can use this computer ----
        // A code nobody scanned isn't a device; the list holds only what actually paired.
        let mut peers: Vec<_> = self
            .peers
            .iter()
            .filter(|peer| peer.paired_at.is_some())
            .cloned()
            .collect();
        peers.sort_by_key(|peer| std::cmp::Reverse(peer.paired_at));
        let device_rows: Vec<AnyElement> = peers
            .into_iter()
            .enumerate()
            .map(|(index, peer)| {
                let id = peer.id.clone();
                let paired = peer.paired_at.is_some();
                let when = peer
                    .paired_at
                    .and_then(|at| chrono::DateTime::from_timestamp(at as i64, 0))
                    .map(|at| {
                        at.with_timezone(&chrono::Local)
                            .format("%b %-d")
                            .to_string()
                    });
                div()
                    .flex()
                    .items_center()
                    .min_h(px(52.))
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(8.))
                    .when(index > 0, |v| {
                        v.border_t_1().border_color(s::card_border().opacity(0.4))
                    })
                    .child(
                        div()
                            .size(px(30.))
                            .flex_none()
                            .rounded_full()
                            .bg(s::frame_bg())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                r::glyph("smartphone")
                                    .size(px(15.))
                                    .text_color(theme.text_muted),
                            ),
                    )
                    .child(
                        r::details(
                            peer.label.clone(),
                            if paired {
                                when.map(|w| format!("Paired {w}"))
                                    .unwrap_or_else(|| "Paired".into())
                            } else {
                                "Waiting for the device to scan the code".into()
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.request(methods::REVOKE_PAIRING, json!({"pairId": id}), cx);
                            cx.notify();
                        })),
                    )
                    .into_any_element()
            })
            .collect();

        let add_computer = r::add_computer_button("native-open-paired-workspace")
            .on_click(
                cx.listener(|this, _, window, cx| this.open_dialog(Dialog::Computer, window, cx)),
            )
            .into_any_element();

        // ---- Advanced: browser access over HTTPS and Tailscale HTTPS, behind a quiet link ----
        let advanced_toggle = div()
            .id("advanced-connection-settings")
            .px(px(4.))
            .text_size(px(12.))
            .text_color(theme.text_muted)
            .cursor_pointer()
            .hover(|v| v.text_color(theme.text))
            .role(Role::Button)
            .child(if self.advanced {
                "Hide advanced settings"
            } else {
                "Advanced settings"
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.advanced = !this.advanced;
                cx.notify();
            }))
            .into_any_element();
        let advanced = self.advanced.then(|| {
            s::card()
                .child(
                    widgets::card_row(&theme, true)
                        .child(details(
                            &theme,
                            "Browser access",
                            &if browser_running {
                                format!("On · {}", self.status["origin"].as_str().unwrap_or("running"))
                            } else {
                                "Serve the Zuse web app at an address and certificate you choose.".to_owned()
                            },
                        ))
                        .child(
                            r::small_button("start-browser-access", if busy { "Updating…" } else if browser_running { "Turn off" } else { "Set up" })
                                .when(busy || browser_running && !managed, |b| b.opacity(0.5).cursor_default())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if busy || browser_running && !managed {
                                        return;
                                    }
                                    this.open_dialog(Dialog::Network, window, cx);
                                })),
                        ),
                )
                .when(direct_running && tailscale_available, |v| {
                    v.child(
                        widgets::card_row(&theme, false)
                            .child(details(&theme, "Tailscale HTTPS", "Also publish on your tailnet with a trusted certificate, so browsers can use it."))
                            .child(
                                widgets::toggle_switch(&theme, direct_serve, "native-tailscale-serve")
                                    .id("native-tailscale-serve")
                                    .role(Role::Switch)
                                    .aria_label("Publish over Tailscale HTTPS")
                                    .aria_toggled(if direct_serve { gpui::Toggled::True } else { gpui::Toggled::False })
                                    .tab_index(0)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.request(methods::START_DIRECT_ACCESS, json!({"tailscaleServe": !direct_serve}), cx)
                                    })),
                            ),
                    )
                })
                .into_any_element()
        });

        let pair_dialog =
            matches!(self.dialog, Some(Dialog::Pair)).then(|| self.pair_dialog(&theme, cx));
        let dialog = pair_dialog.or_else(|| self.dialog.clone().filter(|d| !matches!(d, Dialog::Pair)).map(|dialog| {
            let (title, description) = match &dialog {
                Dialog::Network if browser_running => ("Turn off browser access?", "Browsers using this address will disconnect. You can turn it on again at any time."),
                Dialog::Network => ("Set up browser access", "Choose the address and optional HTTPS configuration browsers will use."),
                Dialog::Computer => ("Add computer", "Paste a pairing link from the other computer to open its workspace."),
                Dialog::Pair => unreachable!("rendered by pair_dialog"),
            };
            let mut body = r::dialog_body();
            match &dialog {
                Dialog::Network if !browser_running => {
                    body = body
                        .child(r::details("Listen address", "Use localhost for this computer or a LAN address with trusted HTTPS."))
                        .child(field(&theme, self.listen.clone()))
                        .child(r::details("Public URL", "HTTPS URL used by your other devices. Optional for localhost."))
                        .child(field(&theme, self.origin.clone()))
                        .child(r::details("HTTPS certificate and key", "File paths on this computer. Leave blank for a local connection."))
                        .child(field(&theme, self.certificate.clone()))
                        .child(field(&theme, self.key.clone()));
                }
                Dialog::Computer => body = body.child(field(&theme, self.destination.clone())),
                _ => {}
            }
            body = body.when_some(self.error.clone(), |v, error| v.child(widgets::error_strip(&theme, error)));
            let caption = if busy {
                "Updating…"
            } else {
                match dialog {
                    Dialog::Network if browser_running => "Turn off",
                    Dialog::Network => "Turn on",
                    Dialog::Computer => "Connect",
                    Dialog::Pair => "Done",
                }
            };
            let footer = r::dialog_footer()
                .child(
                    r::button("close-remote-dialog", crate::localization::text("settings-organizations_cancel", cx))
                        .track_focus(&self.dialog_focus)
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                )
                .child(r::button("confirm-remote-dialog", caption).when(busy, |v| v.opacity(0.5)).on_click(cx.listener(move |this, _, window, cx| {
                    if this.busy {
                        return;
                    }
                    match &dialog {
                        Dialog::Network if browser_running => this.request(methods::STOP_BROWSER_ACCESS, json!({}), cx),
                        Dialog::Network => this.start(cx),
                        Dialog::Computer => this.open_destination(window, cx),
                        Dialog::Pair => {}
                    }
                })));
            r::Modal(
                r::dialog_panel("native-remote-dialog", title, description)
                    .track_focus(&self.dialog_root)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.close_dialog(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(body)
                    .child(footer)
                    .into_any_element(),
            )
            .into_any_element()
        }));
        let hero = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(hero)
            .when(self.dialog.is_none(), |v| {
                v.when_some(self.error.clone(), |v, error| {
                    v.child(widgets::error_strip(&theme, error))
                })
            })
            .into_any_element();
        PairingParts {
            hero,
            device_rows,
            add_computer,
            advanced_toggle,
            advanced,
            dialog,
        }
    }

    /// The pairing dialog: a title, two quiet tabs, and the code with nothing boxed around it.
    fn pair_dialog(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use zuse_viewport::{remote_access_style as r, settings_style as s};
        let link = self
            .invitation
            .as_ref()
            .and_then(|target| target.export_link().ok());
        let paired = self.paired_code().map(|peer| peer.label.clone());
        let remaining = self
            .code_created
            .map(|at| 600u64.saturating_sub(at.elapsed().as_secs()));
        let expired = remaining == Some(0);
        let tab = self.pair_tab;
        let muted = theme.text_muted;

        // Text tabs with a thin underline on the active one.
        let tab_item = |index: usize, label: &'static str, cx: &mut Context<Self>| {
            let active = tab == index;
            div()
                .id(("pair-tab", index))
                .pb(px(8.))
                .border_b_2()
                .border_color(if active {
                    theme.text
                } else {
                    gpui::transparent_black()
                })
                .text_size(px(13.))
                .cursor_pointer()
                .role(Role::Tab)
                .text_color(if active { theme.text } else { muted })
                .when(active, |v| v.font_weight(FontWeight::MEDIUM))
                .when(!active, |v| v.hover(|v| v.text_color(theme.text)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pair_tab = index;
                    cx.notify();
                }))
        };
        let tabs = div()
            .w_full()
            .flex()
            .gap(px(20.))
            .border_b_1()
            .border_color(s::card_border().opacity(0.5))
            .child(tab_item(0, "Phone", cx))
            .child(tab_item(1, "Computer", cx));

        let text_action = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .text_size(px(12.))
                .text_color(muted)
                .cursor_pointer()
                .role(Role::Button)
                .hover(|v| v.text_color(theme.text))
                .child(label)
        };
        let countdown =
            div()
                .text_size(px(12.))
                .text_color(theme.text_faint)
                .child(SharedString::from(match remaining {
                    Some(0) => "Code expired".to_owned(),
                    Some(left) => format!("Expires in {}:{:02}", left / 60, left % 60),
                    None => "Creating a code…".to_owned(),
                }));
        let copy = link.clone().filter(|_| !expired);
        let footer_row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .child(countdown)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .when_some(copy, |v, link| {
                        v.child(text_action("native-copy-invitation", "Copy link").on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                            },
                        ))
                    })
                    .child(
                        text_action("native-new-invitation", "New code")
                            .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                    ),
            );

        let content: AnyElement = if let Some(name) = paired.clone() {
            div()
                .h(px(300.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(
                    div()
                        .size(px(52.))
                        .rounded_full()
                        .bg(theme.success.opacity(0.15))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(r::glyph("check").size(px(24.)).text_color(theme.success)),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(SharedString::from(format!("{name} is connected"))),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(muted)
                        .child("It reconnects on its own."),
                )
                .into_any_element()
        } else if tab == 0 {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(16.))
                .child(match (&link, expired) {
                    (Some(link), false) => qr_code(link, 232.),
                    _ => div()
                        .size(px(232.))
                        .rounded(px(20.))
                        .border_1()
                        .border_dashed()
                        .border_color(s::card_border())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(muted)
                        .child(if self.busy {
                            "Creating a code…"
                        } else {
                            "Press New code"
                        })
                        .into_any_element(),
                })
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.text)
                        .text_center()
                        .child("Open Zuse on your phone, tap Add computer, and scan."),
                )
                .child(footer_row)
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(div().text_size(px(13.)).text_color(theme.text).child("On the other computer, open Remote access, choose Add computer, and paste this link."))
                .child(
                    div()
                        .h(px(36.))
                        .px(px(12.))
                        .rounded(px(10.))
                        .bg(s::frame_bg())
                        .flex()
                        .items_center()
                        .truncate()
                        .text_size(px(12.))
                        .font_family("monospace")
                        .text_color(muted)
                        .child(SharedString::from(
                            link.as_deref()
                                .map(|l| l.split('#').next().unwrap_or(l).to_owned() + "#pair=•••••")
                                .unwrap_or_else(|| "Creating a link…".into()),
                        )),
                )
                .child(footer_row)
                .into_any_element()
        };

        let close = div()
            .id("close-remote-dialog")
            .track_focus(&self.dialog_focus)
            .size(px(26.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .role(Role::Button)
            .aria_label("Close")
            .hover(|v| v.bg(s::frame_bg()))
            .child(
                crate::icons::icon(crate::icons::CLOSE)
                    .size(px(13.))
                    .text_color(muted),
            )
            .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx)));

        let panel = div()
            .id("native-pair-dialog")
            .role(Role::Dialog)
            .aria_label("Pair a device")
            .tab_group()
            .w(px(420.))
            .max_w(relative(1.))
            .max_h(relative(0.92))
            .overflow_y_scroll()
            .relative()
            .rounded(px(18.))
            .border_1()
            .border_color(s::card_border())
            .bg(s::card_bg())
            .flex()
            .flex_col()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .track_focus(&self.dialog_root)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_dialog(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(phone_peek(theme))
            .child(
                div()
                    .px(px(22.))
                    .pb(px(22.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child("Pair a device"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(muted)
                                    .child("End-to-end encrypted. The code works once."),
                            ),
                    )
                    .child(tabs)
                    .child(content)
                    .when_some(self.error.clone(), |v, error| {
                        v.child(widgets::error_strip(theme, error))
                    }),
            )
            .child(div().absolute().top(px(12.)).right(px(12.)).child(close));
        r::Modal(panel.into_any_element()).into_any_element()
    }
}

/// The top of a phone showing the Add computer screen, fading into the dialog. Drawn like a real
/// handset: dark frame with a lit edge, slim bezel, island, status bar and side buttons.
fn phone_peek(theme: &Theme) -> Div {
    use zuse_viewport::settings_style as s;
    let body = gpui::rgb(0x0b0b0c);
    let edge = gpui::white().opacity(0.10);
    let screen = s::card_bg();
    let ink = theme.text;
    let faint = theme.text_muted;
    let bracket = |v: Div| v.absolute().size(px(10.)).border_color(ink.opacity(0.8));
    let side_button = |left: bool, top: f32, height: f32| {
        div()
            .absolute()
            .top(px(top))
            .when(left, |v| v.left(px(-3.)))
            .when(!left, |v| v.right(px(-3.)))
            .w(px(3.))
            .h(px(height))
            .rounded(px(2.))
            .bg(gpui::rgb(0x1a1a1c))
    };
    let phone = div()
        .relative()
        .w(px(184.))
        .h(px(300.))
        .rounded(px(38.))
        .bg(body)
        .border_1()
        .border_color(edge)
        .p(px(6.))
        .shadow_lg()
        .child(side_button(true, 64., 18.))
        .child(side_button(true, 92., 32.))
        .child(side_button(false, 84., 44.))
        .child(
            div()
                .relative()
                .size_full()
                .rounded(px(32.))
                .bg(screen)
                .overflow_hidden()
                .flex()
                .flex_col()
                // Status bar with the island between the time and the battery.
                .child(
                    div()
                        .h(px(30.))
                        .px(px(18.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(9.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ink)
                                .child("9:41"),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .children(
                                    [4., 6., 8.]
                                        .map(|h| div().w(px(2.)).h(px(h)).rounded(px(1.)).bg(ink)),
                                )
                                .child(
                                    div()
                                        .ml(px(3.))
                                        .w(px(16.))
                                        .h(px(8.))
                                        .rounded(px(2.5))
                                        .border_1()
                                        .border_color(ink.opacity(0.5))
                                        .p(px(1.))
                                        .child(div().w(px(10.)).h_full().rounded(px(1.)).bg(ink)),
                                ),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(8.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(div().w(px(54.)).h(px(16.)).rounded_full().bg(body)),
                )
                // The Add computer screen.
                .child(
                    div()
                        .px(px(14.))
                        .pt(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(ink)
                                .child("Add computer"),
                        )
                        .child(
                            div()
                                .h(px(96.))
                                .rounded(px(14.))
                                .bg(s::frame_bg())
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .size(px(36.))
                                        .relative()
                                        .child(bracket(
                                            div()
                                                .top_0()
                                                .left_0()
                                                .border_t_2()
                                                .border_l_2()
                                                .rounded_tl(px(3.)),
                                        ))
                                        .child(bracket(
                                            div()
                                                .top_0()
                                                .right_0()
                                                .border_t_2()
                                                .border_r_2()
                                                .rounded_tr(px(3.)),
                                        ))
                                        .child(bracket(
                                            div()
                                                .bottom_0()
                                                .left_0()
                                                .border_b_2()
                                                .border_l_2()
                                                .rounded_bl(px(3.)),
                                        ))
                                        .child(bracket(
                                            div()
                                                .bottom_0()
                                                .right_0()
                                                .border_b_2()
                                                .border_r_2()
                                                .rounded_br(px(3.)),
                                        ))
                                        .child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    crate::icons::icon(crate::icons::DITHER_QR)
                                                        .size(px(16.))
                                                        .text_color(ink),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ink)
                                        .child("Scan QR code"),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(8.))
                                .text_color(faint)
                                .child("Or paste a link"),
                        ),
                ),
        );
    div()
        .relative()
        .h(px(172.))
        .w_full()
        .overflow_hidden()
        .rounded_t(px(18.))
        .bg(gpui::linear_gradient(
            180.,
            gpui::linear_color_stop(theme.accent.opacity(0.12), 0.),
            gpui::linear_color_stop(theme.accent.opacity(0.0), 1.),
        ))
        .child(
            div()
                .absolute()
                .top(px(26.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(phone),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(64.))
                .bg(gpui::linear_gradient(
                    180.,
                    gpui::linear_color_stop(s::card_bg().opacity(0.), 0.),
                    gpui::linear_color_stop(s::card_bg(), 1.),
                )),
        )
}
