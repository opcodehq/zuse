//! Sidebar account and provider limits, shared by both hosts.
use crate::localization::message as tr;
use crate::{assets::icon, creation_style as ui, palette, settings_style as s};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};
use zuse_i18n::messages as msg;
use zuse_protocol::{AgentAccount, AgentAccountsSnapshot, HarnessId, methods};

#[derive(Clone, PartialEq)]
pub struct Properties {
    pub account: Option<Value>,
    pub available: bool,
    pub target: Option<String>,
    pub working: usize,
}
pub enum Event {
    Call {
        id: u64,
        method: &'static str,
        params: Value,
    },
    Cancel(u64),
    CloudSettings,
    FullUsage,
}
type Owner = (String, Option<String>);
pub struct SidebarControls {
    properties: Properties,
    owner: Option<Owner>,
    next: u64,
    requests: BTreeMap<u64, (&'static str, Option<Owner>)>,
    subscribed: Option<bool>,
    plan_error: Option<String>,
    account_error: Option<String>,
    accounts: AgentAccountsSnapshot,
    usage_loaded_at: Option<web_time::Instant>,
    usage_error: Option<String>,
    usage_open: bool,
    restore_usage_focus: bool,
    active: usize,
    detail: Option<usize>,
    account_focus: FocusHandle,
    usage_focus: FocusHandle,
    menu_focus: FocusHandle,
    usage_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    menu_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    detail_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}
impl EventEmitter<Event> for SidebarControls {}
impl SidebarControls {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            properties: Properties {
                account: None,
                available: false,
                target: None,
                working: 0,
            },
            owner: None,
            next: 0,
            requests: Default::default(),
            subscribed: None,
            plan_error: None,
            account_error: None,
            accounts: Default::default(),
            usage_loaded_at: None,
            usage_error: None,
            usage_open: false,
            restore_usage_focus: false,
            active: 0,
            detail: None,
            account_focus: cx.focus_handle().tab_stop(true),
            usage_focus: cx.focus_handle().tab_stop(true),
            menu_focus: cx.focus_handle(),
            usage_bounds: Default::default(),
            menu_bounds: Default::default(),
            detail_bounds: Default::default(),
        }
    }
    pub fn set_properties(&mut self, properties: Properties, cx: &mut Context<Self>) {
        if self.properties == properties {
            return;
        }
        let owner = properties.account.as_ref().and_then(|a| {
            Some((
                a["user"]["id"].as_str()?.to_owned(),
                a["orgId"].as_str().map(str::to_owned),
            ))
        });
        let account_changed = owner != self.owner;
        let target_changed = properties.target != self.properties.target;
        if account_changed || target_changed || !properties.available {
            self.restore_usage_focus |= self.usage_open;
            for id in self.requests.keys().copied().collect::<Vec<_>>() {
                cx.emit(Event::Cancel(id));
            }
            self.requests.clear();
            self.usage_loaded_at = None;
            self.accounts = Default::default();
            self.usage_error = None;
            self.usage_open = false;
            self.detail = None;
        }
        if account_changed || !properties.available {
            self.subscribed = None;
            self.plan_error = None;
            self.account_error = None;
        }
        self.owner = owner;
        self.properties = properties;
        if self.owner.is_some()
            && self.properties.available
            && (account_changed || self.subscribed.is_none())
            && !self.pending("plan")
        {
            self.request("plan", methods::CLOUD_BILLING_ENTITLEMENTS, json!({}), cx);
        }
        cx.notify();
    }
    fn pending(&self, kind: &str) -> bool {
        self.requests.values().any(|(k, _)| *k == kind)
    }
    fn request(
        &mut self,
        kind: &'static str,
        method: &'static str,
        mut params: Value,
        cx: &mut Context<Self>,
    ) {
        if !self.properties.available || self.pending(kind) {
            return;
        }
        if matches!(kind, "plan" | "checkout") {
            let Some((account, organization)) = &self.owner else {
                return;
            };
            params["expectedAccountId"] = json!(account);
            params["expectedOrganizationId"] = json!(organization);
        }
        self.next += 1;
        let id = self.next;
        self.requests.insert(id, (kind, self.owner.clone()));
        cx.emit(Event::Call { id, method, params });
        cx.notify();
    }
    pub fn receive(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some((kind, owner)) = self.requests.remove(&id) else {
            return;
        };
        if owner != self.owner {
            return;
        }
        let result = result.and_then(|value| {
            if matches!(kind, "plan" | "checkout")
                && !owner.as_ref().is_some_and(|(a, o)| {
                    value["accountId"].as_str() == Some(a.as_str())
                        && value["organizationId"].as_str() == o.as_deref()
                })
            {
                Err("The account changed. Reopen Cloud settings to refresh.".into())
            } else {
                Ok(value)
            }
        });
        match (kind, result) {
            ("plan", Ok(value)) => {
                self.subscribed = zuse_model::cloud_billing::cloud_subscribed(
                    &value["data"],
                    chrono::Utc::now().timestamp_millis(),
                );
                self.plan_error = self
                    .subscribed
                    .is_none()
                    .then(|| "The Cloud plan is unavailable. Open Cloud settings to retry.".into());
            }
            ("plan", Err(error)) => self.plan_error = Some(error),
            ("usage", Ok(value)) => match serde_json::from_value(value) {
                Ok(accounts) => {
                    self.accounts = accounts;
                    self.usage_loaded_at = Some(web_time::Instant::now());
                    self.usage_error = None;
                }
                Err(_) => {
                    self.usage_error =
                        Some("Could not read provider usage. Refresh to retry.".into())
                }
            },
            ("usage", Err(error)) => {
                self.usage_loaded_at = Some(web_time::Instant::now());
                self.usage_error = Some(error);
            }
            ("signin" | "checkout", Ok(value)) => {
                let url = if kind == "signin" {
                    value["url"].as_str()
                } else {
                    value["data"]["checkoutUrl"].as_str()
                };
                if let Some(url) = url.filter(|url| url.starts_with("https://")) {
                    cx.open_url(url);
                } else {
                    self.account_error = Some(
                        "No secure sign-in or checkout link was returned. Open Settings to retry."
                            .into(),
                    );
                }
            }
            (_, Err(error)) => self.account_error = Some(error),
            _ => {}
        }
        cx.notify();
    }
    pub fn interrupted(&mut self, cx: &mut Context<Self>) {
        for id in self.requests.keys().copied().collect::<Vec<_>>() {
            cx.emit(Event::Cancel(id));
        }
        self.requests.clear();
        self.usage_loaded_at = None;
        self.restore_usage_focus |= self.usage_open;
        self.usage_open = false;
        self.detail = None;
        self.properties.available = false;
        self.subscribed = None;
        self.plan_error = None;
        cx.notify();
    }
    fn account_action(&mut self, cx: &mut Context<Self>) {
        if !self.properties.available
            || self.properties.account.is_none()
            || self
                .properties
                .account
                .as_ref()
                .is_some_and(|a| a["login"].is_object())
            || self.pending("signin")
            || self.pending("checkout")
        {
            return;
        }
        self.account_error = None;
        if self.owner.is_none() {
            self.request("signin", methods::CLOUD_ACCOUNT_SIGN_IN, json!({}), cx);
        } else if self.subscribed == Some(false) && self.plan_error.is_none() {
            self.request(
                "checkout",
                methods::CLOUD_BILLING_CHECKOUT,
                json!({"offerId":"cloud-workspace-standard-v1"}),
                cx,
            );
        } else if self.subscribed == Some(true) || self.plan_error.is_some() {
            cx.emit(Event::CloudSettings);
        }
    }
    fn load_usage(&mut self, force: bool, cx: &mut Context<Self>) {
        let retry = self.usage_error.is_some()
            || providers().iter().any(|(h, _, _)| {
                !self.accounts.accounts.iter().any(|a| {
                    a.harness == *h
                        && a.active
                        && !a.usage_windows.is_empty()
                        && a.usage_error.is_none()
                })
            });
        let stale = self
            .usage_loaded_at
            .is_none_or(|at| at.elapsed().as_secs() >= if retry { 5 } else { 60 });
        if (stale || force) && !self.pending("usage") {
            let mut params = json!({"forceUsage":force});
            if let Some(target) = &self.properties.target {
                params["targetDeviceId"] = json!(target);
            }
            self.request("usage", methods::LIST_AGENT_ACCOUNTS, params, cx);
        }
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage_open = false;
        self.detail = None;
        self.detail_bounds.set(None);
        self.menu_bounds.set(None);
        window.focus(&self.usage_focus, cx);
        cx.notify();
    }
    pub fn dismiss_on_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.usage_open {
            return false;
        }
        self.close(window, cx);
        true
    }
    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.usage_open {
            self.close(window, cx);
        } else {
            self.usage_open = true;
            self.active = usize::MAX;
            self.detail = None;
            self.load_usage(false, cx);
            window.focus(&self.menu_focus, cx);
            cx.notify();
        }
    }
    fn provider(&self, index: usize) -> Option<&AgentAccount> {
        self.accounts
            .accounts
            .iter()
            .find(|a| a.harness == providers()[index].0 && a.active)
    }
    fn usage_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = providers().len() + 2;
        match e.keystroke.key.as_str() {
            "escape" | "tab" => {
                self.close(window, cx);
                if e.keystroke.key == "tab" {
                    return;
                }
            }
            "up" => {
                self.active = if self.active == usize::MAX {
                    count - 1
                } else {
                    (self.active + count - 1) % count
                };
                self.detail = None;
            }
            "down" => {
                self.active = if self.active == usize::MAX {
                    0
                } else {
                    (self.active + 1) % count
                };
                self.detail = None;
            }
            "home" => {
                self.active = 0;
                self.detail = None;
            }
            "end" => {
                self.active = count - 1;
                self.detail = None;
            }
            "left" => self.detail = None,
            "right" | "enter" | "space" if self.active < providers().len() => {
                self.detail = Some(self.active)
            }
            "enter" | "space" if self.active == providers().len() => self.load_usage(true, cx),
            "enter" | "space" if self.active == providers().len() + 1 => {
                self.close(window, cx);
                cx.emit(Event::FullUsage);
            }
            "enter" | "space" => {}
            _ => return,
        }
        if self.detail.is_none() {
            self.detail_bounds.set(None);
        }
        cx.stop_propagation();
        cx.notify();
    }
    fn provider_detail(&self, index: usize) -> AnyElement {
        let (_, name, glyph) = providers()[index];
        let account = self.provider(index);
        let mut view = menu_surface(320.).px(px(11.)).py(px(9.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pb(px(8.))
                .child(icon(glyph).size(px(16.)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(name),
                        )
                        .child(
                            ui::meta(
                                account
                                    .and_then(|a| a.plan_label.clone())
                                    .unwrap_or_else(|| "Usage limits".into()),
                            )
                            .text_size(px(11.))
                            .line_height(px(14.85)),
                        ),
                ),
        );
        if let Some(error) = account.and_then(|a| a.usage_error.clone()) {
            view = view.child(div().text_size(px(12.)).pb(px(8.)).child(error));
        }
        if let Some(account) = account.filter(|a| !a.usage_windows.is_empty()) {
            for window in &account.usage_windows {
                let used = window.used_fraction.clamp(0., 1.);
                let left = ((1. - used) * 100.).round() as u32;
                let reset = window
                    .resets_at
                    .map(|at| {
                        let minutes = (at - chrono::Utc::now()).num_minutes().max(0);
                        if at <= chrono::Utc::now() {
                            "Reset unavailable".into()
                        } else if minutes >= 1440 {
                            format!("Resets in {}d {}h", minutes / 1440, (minutes / 60) % 24)
                        } else if minutes >= 60 {
                            format!("Resets in {}h {:02}m", minutes / 60, minutes % 60)
                        } else {
                            format!("Resets in {minutes}m")
                        }
                    })
                    .unwrap_or_else(|| "Reset unavailable".into());
                view = view.child(
                    div()
                        .py(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(window.label.clone()),
                                )
                                .child(
                                    ui::meta(format!("{left}% left"))
                                        .text_size(px(12.))
                                        .line_height(px(16.)),
                                ),
                        )
                        .child(stick_meter(left as f32, used >= 0.8))
                        .child(ui::meta(reset).text_size(px(11.)).line_height(px(14.85))),
                );
            }
        } else {
            view = view.child(
                div()
                    .py(px(24.))
                    .text_size(px(12.))
                    .text_center()
                    .text_color(palette::rgb(crate::shell::MUTED))
                    .child(if self.pending("usage") {
                        "Loading usage…"
                    } else if account.is_some() {
                        "Usage limits are unavailable for this account."
                    } else {
                        "Sign in to this provider to see usage limits."
                    }),
            );
        }
        let bounds = self.detail_bounds.clone();
        crate::glass::menu(
            view.child(
                canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            ),
        )
        .into_any_element()
    }
    fn usage_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let menu_bounds = self.menu_bounds.clone();
        let detail_bounds = self.detail_bounds.clone();
        let measured = self.menu_bounds.clone();
        let mut menu = menu_surface(288.)
            .p(px(3.))
            .id("sidebar-usage-menu")
            .role(Role::Menu)
            .track_focus(&self.menu_focus)
            .occlude()
            .capture_key_down(cx.listener(Self::usage_key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                if ![menu_bounds.get(), detail_bounds.get()]
                    .iter()
                    .flatten()
                    .any(|b| b.contains(&e.position))
                {
                    this.close(window, cx);
                }
            }))
            .child(
                canvas(move |b, _, _| measured.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(tr(cx, msg::usage::usage_dashboard_usage_limits)),
            );
        if let Some(error) = &self.usage_error {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .child(error.clone()),
            );
        }
        if self.pending("usage") {
            menu = menu.child(
                ui::meta("Loading usage…")
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(12.)),
            );
        }
        for (i, (_, name, glyph)) in providers().into_iter().enumerate() {
            let summary = self
                .provider(i)
                .and_then(|a| a.usage_windows.first())
                .map(|w| {
                    format!(
                        "{}% left",
                        ((1. - w.used_fraction.clamp(0., 1.)) * 100.).round() as u32
                    )
                });
            let mut option = div()
                .id(("sidebar-usage-provider", i))
                .role(Role::MenuItem)
                .tab_index(0)
                .relative()
                .h(px(28.))
                .min_w_0()
                .flex()
                .items_center()
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(10.))
                .text_size(px(14.))
                .line_height(px(20.))
                .text_color(palette::rgb(crate::shell::TEXT))
                .cursor_pointer()
                .hover(|v| v.bg(s::frame_bg()))
                .child(icon(glyph).size(px(14.)).mx(px(-2.)))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .when(self.active == i, |v| v.bg(s::frame_bg().opacity(0.6)))
                .when_some(summary, |v, label| {
                    v.child(ui::meta(label).text_size(px(11.)))
                })
                .child(
                    icon("alt-arrow-right")
                        .size(px(16.))
                        .mr(px(-2.))
                        .opacity(0.8),
                )
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    if *hovered {
                        this.active = i;
                        this.detail = Some(i);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active = i;
                    this.detail = Some(i);
                    cx.notify();
                }));
            if self.detail == Some(i) {
                option = option.child(
                    div().absolute().left_full().top(px(-5.)).child(deferred(
                        anchored()
                            .anchor(Anchor::TopLeft)
                            .snap_to_window_with_margin(px(8.))
                            .child(self.provider_detail(i)),
                    )),
                );
            }
            menu = menu.child(option);
        }
        crate::glass::menu(
            menu.child(ui::separator())
                .child(
                    ui::option("sidebar-usage-refresh", "Refresh usage", "", false)
                        .role(Role::MenuItem)
                        .when(self.active == providers().len(), |v| {
                            v.bg(s::frame_bg().opacity(0.6))
                        })
                        .when(self.pending("usage"), |v| v.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.load_usage(true, cx))),
                )
                .child(
                    ui::option("sidebar-usage-accounts", "Full usage", "analytics", false)
                        .role(Role::MenuItem)
                        .when(self.active == providers().len() + 1, |v| {
                            v.bg(s::frame_bg().opacity(0.6))
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close(window, cx);
                            cx.emit(Event::FullUsage);
                        })),
                ),
        )
        .into_any_element()
    }
}
impl Render for SidebarControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.restore_usage_focus) {
            window.focus(&self.usage_focus, cx);
        }
        let loading = self.properties.account.is_none();
        let signed_in = self.owner.is_some();
        let signing_in = self.pending("signin")
            || self
                .properties
                .account
                .as_ref()
                .is_some_and(|a| a["login"].is_object());
        let disabled = loading
            || signing_in
            || !self.properties.available
            || self.pending("signin")
            || self.pending("checkout")
            || (signed_in && self.subscribed.is_none() && self.plan_error.is_none());
        let label = if loading {
            tr(cx, msg::native::loading_account)
        } else if !self.properties.available {
            tr(cx, msg::native::account_unavailable)
        } else if signing_in {
            "Signing in".to_owned()
        } else {
            account_plan_label(signed_in, self.subscribed, self.plan_error.is_some()).to_owned()
        };
        let upgrade = signed_in && self.subscribed == Some(false) && self.plan_error.is_none();
        let tip = self
            .account_error
            .as_ref()
            .or(self.plan_error.as_ref())
            .cloned()
            .unwrap_or_else(|| label.to_owned());
        let account = footer_button("sidebar-account", label.clone())
            .debug_selector(|| "sidebar-account".into())
            .aria_description(tip.clone())
            .tooltip(move |_, cx| cx.new(|_| ControlTip(tip.clone().into())).into())
            .track_focus(&self.account_focus.clone().tab_stop(!disabled))
            .tab_index(if disabled { -1 } else { 0 })
            .when(disabled, |v| v.opacity(0.5).cursor_default())
            .when(signed_in, |v| {
                v.px(px(8.))
                    .gap(px(6.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(upgrade, |v| {
                v.relative()
                    .overflow_hidden()
                    .child(crate::dither::fill([151, 183, 76], 7.))
                    .text_color(white())
                    .px(px(10.))
                    .text_size(px(11.))
            })
            .when(!signed_in, |v| v.child(icon("login").size(px(16.))))
            .when(label == "Cloud", |v| {
                v.child(icon("cloud-dither").size(px(14.)))
            })
            .child(div().relative().child(label))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    this.account_action(cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.account_action(cx)));
        let mut usage = footer_button("sidebar-usage", "Usage limits")
            .debug_selector(|| "sidebar-usage".into())
            .when(!self.usage_open, |v| {
                v.tooltip(|_, cx| cx.new(|_| ControlTip("Usage limits".into())).into())
            })
            .w(px(28.))
            .relative()
            .track_focus(&self.usage_focus)
            .aria_expanded(self.usage_open)
            .child(icon("analytics").size(px(16.)))
            .child({
                let measured = self.usage_bounds.clone();
                canvas(
                    move |bounds, _, _| measured.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
            })
            .on_hover(cx.listener(|this, hovered, _, cx| {
                if *hovered {
                    this.load_usage(false, cx);
                }
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space" | "up" | "down") {
                    let opening = !this.usage_open;
                    this.toggle(window, cx);
                    if opening {
                        this.active = if e.keystroke.key == "up" {
                            providers().len() + 1
                        } else {
                            0
                        };
                    }
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)));
        if self.usage_open {
            // Match the menu positioner's fallback alignment when the trailing
            // anchor would put the popup outside the window's left edge.
            let leading = self
                .usage_bounds
                .get()
                .is_some_and(|bounds| f32::from(bounds.right()) < 296.);
            usage = usage.child(
                div()
                    .absolute()
                    .bottom_full()
                    .when(leading, |v| v.left_0())
                    .when(!leading, |v| v.right_0())
                    .child(deferred(
                        anchored()
                            .anchor(if leading {
                                Anchor::BottomLeft
                            } else {
                                Anchor::BottomRight
                            })
                            .offset(point(px(0.), px(-4.)))
                            .snap_to_window_with_margin(px(8.))
                            .child(self.usage_menu(cx)),
                    )),
            );
        }

        div()
            .w_full()
            .min_w_0()
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(2.))
            .when(
                !(signed_in && self.subscribed.is_none() && self.plan_error.is_none()),
                |v| v.child(account),
            )
            .child(div().flex_1())
            .child(usage)
            // Last, so the host's update pill lands right beside the count.
            .child(
                div()
                    .id("sidebar-working-agents")
                    .role(Role::Status)
                    .aria_label(format!("{} agents working", self.properties.working))
                    .h(px(28.))
                    .min_w(px(28.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(14.))
                    .line_height(px(14.))
                    .text_color(palette::rgb(crate::shell::MUTED))
                    .child(icon("bot").size(px(14.)))
                    .child(if self.properties.available {
                        self.properties.working.to_string()
                    } else {
                        "—".into()
                    }),
            )
    }
}
fn providers() -> [(HarnessId, &'static str, &'static str); 5] {
    [
        (HarnessId::ClaudeCode, "Claude", "claude-mark"),
        (HarnessId::Codex, "Codex", "openai-mark"),
        (HarnessId::Grok, "Grok", "grok-mark"),
        (HarnessId::Gemini, "Gemini", "gemini-mark"),
        (HarnessId::Kiro, "Kiro", "kiro-mark"),
    ]
}
pub fn footer_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .tab_index(0)
        .h(px(28.))
        .min_w(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .text_color(palette::rgb(crate::shell::MUTED))
        .cursor_pointer()
        .hover(|v| v.bg(s::frame_bg().opacity(0.6)))
        .focus_visible(|v| v.border_1().border_color(palette::rgb(0x89ca21)))
}
fn stick_meter(left: f32, warning: bool) -> Div {
    let lit = (left / 100. * 38.).ceil() as usize;
    div()
        .w_full()
        .h(px(16.))
        .flex()
        .gap(px(2.))
        .children((0..38).map(|i| {
            div().flex_1().h_full().rounded(px(2.)).bg(if i < lit {
                palette::rgb(if warning { 0xfcd34d } else { 0x89ca21 }).opacity(if warning {
                    0.65
                } else {
                    0.7
                })
            } else {
                palette::rgb(crate::shell::MUTED).opacity(0.16)
            })
        }))
}
struct ControlTip(SharedString);
impl Render for ControlTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(280.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(7.))
            .border_1()
            .border_color(s::card_border())
            .bg(s::card_bg())
            .text_color(palette::rgb(crate::shell::TEXT))
            .text_size(px(11.))
            .line_height(px(16.))
            .shadow_sm()
            .child(self.0.clone())
    }
}

fn menu_surface(width: f32) -> Div {
    let dark = !palette::is_light();
    ui::menu(width)
        .relative()
        .border_color(transparent_black())
        .when(dark, |v| v.bg(hsla(132. / 360., 0.05, 0.105, 0.72)))
        .when(dark, |v| {
            v.child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(10.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(white().opacity(0.045), 0.),
                        linear_color_stop(white().opacity(0.008), 1.),
                    ))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(white().opacity(0.065)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(px(8.))
                            .right(px(8.))
                            .h(px(1.))
                            .bg(white().opacity(0.09)),
                    ),
            )
        })
}

fn account_plan_label(signed_in: bool, subscribed: Option<bool>, failed: bool) -> &'static str {
    if !signed_in {
        "Sign in"
    } else if failed {
        "Account"
    } else {
        match subscribed {
            Some(true) => "Cloud",
            Some(false) => "Upgrade",
            None => "Checking plan",
        }
    }
}
#[cfg(test)]
mod account_label_tests {
    use super::account_plan_label;
    #[test]
    fn cloud_badge_requires_a_current_account_and_verified_entitlement() {
        assert_eq!(account_plan_label(false, Some(true), false), "Sign in");
        assert_eq!(account_plan_label(true, None, false), "Checking plan");
        assert_eq!(account_plan_label(true, None, true), "Account");
        assert_eq!(account_plan_label(true, Some(true), true), "Account");
        assert_eq!(account_plan_label(true, Some(false), false), "Upgrade");
        assert_eq!(account_plan_label(true, Some(true), false), "Cloud");
    }
}
