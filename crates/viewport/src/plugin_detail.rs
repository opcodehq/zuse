//! Shared plugin details and connection controls; account operations stay with the host.
use crate::{assets::icon, plugin_catalog, plugin_setup::SetupColors};
use gpui::{prelude::*, *};
use serde_json::Value;
use zuse_protocol::plugins::PluginDefinition;

#[derive(Default)]
pub struct Menu {
    pub open: bool,
    pub account: Option<String>,
    pub removal: Option<Entity<crate::plugin_removal::ServerRemoval>>,
    pub removal_events: Option<Subscription>,
    pub connect_focus: Option<FocusHandle>,
    account_focus: Option<[FocusHandle; 2]>,
    triggers: std::collections::HashMap<String, FocusHandle>,
    closed: Option<web_time::Instant>,
    account_closed: Option<(String, web_time::Instant)>,
}
impl Menu {
    pub fn toggle_account(&mut self, id: String, window: &mut Window, cx: &mut App) {
        self.open = false;
        if self.account_closed.as_ref().is_some_and(|(closed, time)| {
            closed == &id && time.elapsed() < std::time::Duration::from_millis(300)
        }) {
            self.account_closed = None;
            return;
        }
        self.account = if self.account.as_ref() == Some(&id) {
            None
        } else {
            Some(id)
        };
        if self.account.is_some() {
            if let Some(handles) = &self.account_focus {
                window.focus(&handles[0], cx);
            }
        }
    }
    pub fn close_account(&mut self, restore: bool, window: &mut Window, _cx: &mut App) {
        if let Some(id) = self.account.take() {
            self.account_closed = Some((id.clone(), web_time::Instant::now()));
            if !restore {
                return;
            }
            if let Some(focus) = self.triggers.get(&id).cloned() {
                window.on_next_frame(move |window, cx| window.focus(&focus, cx));
            }
        }
    }
    pub fn toggle(&mut self) {
        self.account = None;
        self.open = !self.open
            && !self
                .closed
                .is_some_and(|time| time.elapsed() < std::time::Duration::from_millis(300));
        self.closed = None;
    }
    pub fn close(&mut self) {
        self.account = None;
        self.open = false;
        self.closed = Some(web_time::Instant::now());
    }
}
pub enum Action {
    Back,
    MenuToggle,
    MenuClose,
    AccountMenu(String),
    AccountClose,
    AccountDismiss,
    Connect,
    Toggle { id: String, enabled: bool },
    Disconnect(String),
    RemoveServer,
}

pub fn render<T: 'static>(
    definition: &PluginDefinition,
    snapshot: &Value,
    busy: bool,
    menu: &mut Menu,
    inline: Option<Entity<crate::plugin_setup::PluginSetup>>,
    colors: SetupColors,
    cx: &mut Context<T>,
    action: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> AnyElement {
    let connect_focus = menu
        .connect_focus
        .get_or_insert_with(|| cx.focus_handle())
        .clone();
    let account_focus = menu
        .account_focus
        .get_or_insert_with(|| std::array::from_fn(|_| cx.focus_handle()))
        .clone();
    let asking_key = inline.is_some();
    let can_manage = snapshot["canManage"] != false;
    let available = can_manage && !busy;
    let connections: Vec<_> = snapshot["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["pluginId"] == definition.id)
        .cloned()
        .collect();
    let button = |id: &'static str, label: &'static str| {
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .tab_index(0)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(7.))
            .text_size(px(12.))
            .text_color(colors.text)
            .hover(|view| view.bg(colors.border.opacity(0.4)))
    };
    let section = |title: &'static str, body: AnyElement| {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .border_b_1()
                    .border_color(colors.border.opacity(0.6))
                    .pb(px(8.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(div().pt(px(12.)).child(body))
    };
    let mut accounts = div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(colors.border.opacity(0.6))
        .bg(crate::settings_style::frame_bg().opacity(0.2))
        .px(px(12.));
    for (index, connection) in connections.iter().enumerate() {
        let id = connection["id"].as_str().unwrap_or_default().to_owned();
        let menu_id = id.clone();
        let trigger = menu
            .triggers
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let enabled = connection["enabled"] != false;
        let state = connection["state"].as_str().unwrap_or("connected");
        let label = connection["label"]
            .as_str()
            .unwrap_or(&definition.name)
            .to_owned();
        let date = connection["createdAt"]
            .as_f64()
            .and_then(|milliseconds| chrono::DateTime::from_timestamp_millis(milliseconds as i64))
            .map(|date| date.format("%b %-d, %Y").to_string());
        let status = if !enabled {
            "Off · agents can't use this account".into()
        } else {
            match state {
                "connected" => date
                    .map(|date| format!("Connected {date}"))
                    .unwrap_or_else(|| "Connected".into()),
                "needs-auth" => "Needs authorization".into(),
                "connecting" => "Connecting…".into(),
                _ => "Connection failed".into(),
            }
        };
        let initials = label
            .split_whitespace()
            .take(2)
            .map(|word| word.chars().next().unwrap_or_default())
            .collect::<String>();
        let initials = if initials.chars().count() > 1 {
            initials
        } else {
            label.chars().take(2).collect()
        }
        .to_uppercase();
        let hue = label
            .encode_utf16()
            .fold(0u32, |hash, c| hash.wrapping_mul(31).wrapping_add(c as u32))
            % 360;
        accounts = accounts.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(8.))
                .when(index > 0, |d| {
                    d.border_t_1().border_color(colors.border.opacity(0.5))
                })
                .child(
                    div()
                        .size(px(24.))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(avatar_color(hue as f32, enabled))
                        .text_color(white())
                        .text_size(px(9.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(!enabled, |d| d.opacity(0.4))
                        .child(initials),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .truncate()
                                .line_height(px(20.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if enabled { colors.text } else { colors.muted })
                                .child(label),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .line_height(px(20.))
                                .text_color(colors.muted)
                                .child(status),
                        ),
                )
                .child(
                    crate::settings_style::toggle(
                        ("plugin-detail-enabled", index),
                        enabled,
                        "Enable connection",
                    )
                    .when(!available, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if available {
                            action(
                                this,
                                Action::Toggle {
                                    id: id.clone(),
                                    enabled: !enabled,
                                },
                                window,
                                cx,
                            );
                        }
                    })),
                )
                .when(can_manage,|row|row.child(div().relative()
                    .child(div().id(("plugin-account-actions",index)).role(Role::Button)
                        .aria_label(format!("Account actions: {}",connection["label"].as_str().unwrap_or(&definition.name)))
                        .track_focus(&trigger).tab_index(0).cursor_pointer().size(px(28.)).mr(px(-4.))
                        .flex().items_center().justify_center().rounded(px(7.))
                        .hover(|d|d.bg(colors.border.opacity(0.4)))
                        .when(!available,|d|d.opacity(0.5))
                        .child(icon("more-horizontal").size(px(16.)).text_color(colors.muted))
                        .on_mouse_down(MouseButton::Left,|_,_,cx|cx.stop_propagation())
                        .on_click(cx.listener({let id=menu_id.clone();move|this,_,window,cx|if available {action(this,Action::AccountMenu(id.clone()),window,cx)}}))
                        .on_key_down(cx.listener({let id=menu_id.clone();move|this,event:&KeyDownEvent,window,cx| {
                            if matches!(event.keystroke.key.as_str(),"enter"|"space"|"down") {if available {action(this,Action::AccountMenu(id.clone()),window,cx)}cx.stop_propagation();}
                        }})))
                    .when(menu.account.as_ref()==Some(&menu_id),|d|d.child(deferred(
                        div().id("plugin-account-menu").role(Role::Menu).absolute().top_full().right_0().mt(px(4.))
                            .w(px(192.)).p(px(4.)).rounded(px(10.)).border_1().border_color(colors.border)
                            .bg(crate::palette::rgb(crate::shell::RAISED)).shadow_lg().occlude()
                            .on_mouse_down_out(cx.listener(move|this,_,window,cx|action(this,Action::AccountDismiss,window,cx)))
                            .on_key_down(cx.listener({let handles=account_focus.clone();move|this,event:&KeyDownEvent,window,cx| {
                                let index=match event.keystroke.key.as_str() {
                                    "escape"=>{action(this,Action::AccountClose,window,cx);cx.stop_propagation();return;},
                                    "home"=>0,"end"=>1,"up"|"down"=>if handles[0].is_focused(window) {1}else{0},_=>return,
                                }; window.focus(&handles[index],cx);cx.stop_propagation();
                            }}))
                            .children([(false,if enabled {"Turn off"}else{"Turn on"}),(true,"Disconnect")].into_iter().enumerate().map(|(i,(remove,title))| {
                                let id=menu_id.clone();let handles=account_focus.clone();
                                button(if remove {"plugin-account-disconnect"} else {"plugin-account-toggle"},title)
                                    .role(Role::MenuItem).track_focus(&handles[i]).w_full().justify_start()
                                    .when(remove,|d|d.text_color(colors.danger).child(icon("link-broken").size(px(14.))))
                                    .child(title)
                                    .on_click(cx.listener({let id=id.clone();move|this,_,window,cx| {
                                        action(this,Action::AccountClose,window,cx);
                                        if available {action(this,if remove {Action::Disconnect(id.clone())}else{Action::Toggle{id:id.clone(),enabled:!enabled}},window,cx)}
                                    }}))
                                    .on_key_down(cx.listener(move|this,event:&KeyDownEvent,window,cx| {
                                        if matches!(event.keystroke.key.as_str(),"enter"|"space") {
                                            action(this,Action::AccountClose,window,cx);
                                            if available {action(this,if remove {Action::Disconnect(id.clone())}else{Action::Toggle{id:id.clone(),enabled:!enabled}},window,cx)}
                                            cx.stop_propagation();
                                        }
                                    }))
                            }))
                    ).priority(2))))),
        )
    }
    if let Some(form) = inline {
        accounts = accounts.child(
            div()
                .when(!connections.is_empty(), |d| {
                    d.border_t_1().border_color(colors.border.opacity(0.5))
                })
                .child(form),
        );
    } else if can_manage && !definition.id.starts_with("local-") {
        accounts = accounts.child(
            div()
                .id("plugin-detail-connect-another")
                .track_focus(&connect_focus)
                .role(Role::Button)
                .aria_label("Connect another account")
                .tab_index(0)
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(8.))
                .when(!connections.is_empty(), |d| {
                    d.border_t_1().border_color(colors.border.opacity(0.5))
                })
                .hover(|d| d.bg(colors.border.opacity(0.3)))
                .when(busy, |d| d.opacity(0.6))
                .child(add_glyph(colors))
                .child("Connect another account")
                .on_click(cx.listener(move |this, _, window, cx| {
                    if available {
                        action(this, Action::Connect, window, cx);
                    }
                })),
        );
    }
    let url = definition.url.clone();
    let subtitle = definition
        .url
        .as_deref()
        .filter(|_| definition.custom)
        .map(|url| {
            url.split("://")
                .nth(1)
                .unwrap_or(url)
                .split('/')
                .next()
                .unwrap_or(url)
        })
        .unwrap_or(&definition.domain)
        .to_owned();
    let website = format!("https://{}", definition.domain);
    let mut actions = div().flex().items_center().gap(px(4.)).when(busy, |d| {
        d.child(
            div()
                .h(px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_color(colors.muted)
                .child(icon("refresh").size(px(14.)))
                .child("Waiting…"),
        )
    });
    if !definition.custom || can_manage {
        let custom = definition.custom;
        actions = actions.child(
            div()
                .relative()
                .child(
                    button("plugin-detail-more", "More actions")
                        .size(px(28.))
                        .px_0()
                        .child(
                            icon("more-horizontal")
                                .size(px(16.))
                                .text_color(colors.muted),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            action(this, Action::MenuToggle, window, cx)
                        })),
                )
                .when(menu.open, |d| {
                    d.child(
                        deferred(
                            div()
                                .absolute()
                                .top_full()
                                .right_0()
                                .mt(px(4.))
                                .w(px(176.))
                                .p(px(4.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(colors.border)
                                .bg(crate::palette::rgb(crate::shell::RAISED))
                                .shadow_lg()
                                .occlude()
                                .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
                                    action(this, Action::MenuClose, window, cx)
                                }))
                                .child(
                                    button(
                                        "plugin-detail-menu-action",
                                        if custom {
                                            "Remove server"
                                        } else {
                                            "Visit website"
                                        },
                                    )
                                    .w_full()
                                    .justify_start()
                                    .child(
                                        icon(if custom {
                                            "trash-bin-minimalistic"
                                        } else {
                                            "link"
                                        })
                                        .size(px(14.)),
                                    )
                                    .child(if custom {
                                        "Remove server"
                                    } else {
                                        "Visit website"
                                    })
                                    .when(custom, |d| d.text_color(colors.danger))
                                    .when(custom && busy, |d| d.opacity(0.5))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            action(this, Action::MenuClose, window, cx);
                                            if custom {
                                                if available {
                                                    action(this, Action::RemoveServer, window, cx);
                                                }
                                            } else {
                                                cx.open_url(&website);
                                            }
                                        },
                                    )),
                                ),
                        )
                        .priority(2),
                    )
                }),
        );
    }
    if definition.custom {
        if let Some(url) = url.clone() {
            actions = actions.child(
                button("plugin-detail-copy-url", "Copy URL")
                    .bg(colors.border.opacity(0.4))
                    .child(icon("copy").size(px(14.)))
                    .child("Copy URL")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(url.clone()))
                    }),
            );
        }
    }
    if connections.is_empty() && can_manage && !asking_key {
        actions = actions.child(
            crate::styling::dither_action("plugin-detail-connect", "Connect", available, true)
                .track_focus(&connect_focus)
                .tab_index(0)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if available {
                        action(this, Action::Connect, window, cx);
                    }
                })),
        );
    }
    let mut view = div()
        .flex()
        .flex_col()
        .gap(px(24.))
        .pb(px(32.))
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(colors.text)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(24.))
                        .child(
                            button("plugin-detail-back", "Plugins")
                                .px_0()
                                .text_size(px(13.))
                                .text_color(colors.muted)
                                .child("Plugins")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    action(this, Action::Back, window, cx)
                                })),
                        )
                        .child(div().text_color(colors.muted.opacity(0.4)).child("/"))
                        .child(div().truncate().child(definition.name.clone())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(
                            plugin_catalog::logo(
                                &definition.domain,
                                &definition.name,
                                48.,
                                crate::settings_style::frame_bg(),
                                colors.muted,
                            )
                            .rounded(px(14.)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(12.))
                                        .min_h(px(28.))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .text_size(px(18.))
                                                        .line_height(px(28.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .truncate()
                                                        .child(definition.name.clone()),
                                                )
                                                .when(definition.custom, |d| {
                                                    d.child(
                                                        div()
                                                            .rounded(px(4.))
                                                            .bg(crate::settings_style::frame_bg())
                                                            .px(px(4.))
                                                            .py(px(1.))
                                                            .text_size(px(10.))
                                                            .line_height(px(16.))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(colors.muted)
                                                            .child("Custom"),
                                                    )
                                                }),
                                        )
                                        .child(actions),
                                )
                                .when(!subtitle.is_empty(), |d| {
                                    d.child(
                                        div().truncate().text_color(colors.muted).child(subtitle),
                                    )
                                }),
                        ),
                ),
        )
        .when(!definition.description.is_empty() || !can_manage, |d| {
            d.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when(!definition.description.is_empty(), |d| {
                        d.child(
                            div()
                                .max_w(px(560.))
                                .text_color(colors.text.opacity(0.85))
                                .child(definition.description.clone()),
                        )
                    })
                    .when(!can_manage, |d| {
                        d.child(
                            div().text_color(colors.muted).child(
                                "Only workspace administrators can manage these connections.",
                            ),
                        )
                    }),
            )
        })
        .when(!connections.is_empty() || asking_key, |d| {
            d.child(section("Connected accounts", accounts.into_any_element()))
        });
    let info_row = |label: &'static str, value: AnyElement| {
        div()
            .flex()
            .items_baseline()
            .child(
                div()
                    .w(px(128.))
                    .flex_none()
                    .text_color(colors.muted)
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(value))
    };
    let mut info = div().flex().flex_col().gap(px(8.));
    if definition.custom {
        info = info
            .child(info_row(
                "Server URL",
                div()
                    .truncate()
                    .font_family("Geist Mono")
                    .text_size(px(12.))
                    .child(url.unwrap_or_default())
                    .into_any_element(),
            ))
            .child(info_row(
                "Authentication",
                div()
                    .child(
                        definition
                            .api_key_header
                            .as_ref()
                            .map(|header| {
                                if header.eq_ignore_ascii_case("Authorization") {
                                    "Bearer token".into()
                                } else {
                                    format!("Custom header ({header})")
                                }
                            })
                            .unwrap_or_else(|| "Sign in or none".into()),
                    )
                    .into_any_element(),
            ));
    } else if !definition.domain.is_empty() {
        let website = format!("https://{}", definition.domain);
        info = info.child(info_row(
            "Website",
            div()
                .id("plugin-information-website")
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(definition.domain.clone())
                .child(icon("link").size(px(12.)).text_color(colors.muted))
                .on_click(move |_, _, cx| cx.open_url(&website))
                .into_any_element(),
        ));
    }
    if let Some(category) = &definition.category {
        info = info.child(info_row(
            "Category",
            div().child(category.replace('-', " ")).into_any_element(),
        ));
    }
    info = info.child(info_row(
        "Access",
        div()
            .child("Read and write, within each chat's permissions")
            .into_any_element(),
    ));
    view=view.child(section("Information",div().child(info).child(div().mt(px(16.)).max_w(px(560.)).text_size(px(11.)).line_height(px(16.)).text_color(colors.muted)
        .child(if snapshot["tenantId"].as_str().is_some_and(|id|id.starts_with("organization:")) {
            "Connections are shared with members of this organization. Tokens stay encrypted on Zuse servers and never reach agent machines. Disconnecting stops new calls immediately."
        }else{"Connections are private to your Zuse account. Tokens stay encrypted on Zuse servers and never reach agent machines. Disconnecting stops new calls immediately."})).into_any_element()));
    view.into_any_element()
}

// CSS Color 4 OKLab matrices, matching the reference's label-derived avatar.
fn avatar_color(hue: f32, enabled: bool) -> Rgba {
    let h = hue.to_radians();
    let c = if enabled { 0.14 } else { 0. };
    let a = c * h.cos();
    let b = c * h.sin();
    let l = (0.6 + 0.39633778 * a + 0.21580376 * b).powi(3);
    let m = (0.6 - 0.105561346 * a - 0.06385417 * b).powi(3);
    let s = (0.6 - 0.08948418 * a - 1.2914855 * b).powi(3);
    let gamma = |x: f32| {
        let x = x.clamp(0., 1.);
        if x <= 0.0031308 {
            12.92 * x
        } else {
            1.055 * x.powf(1. / 2.4) - 0.055
        }
    };
    Rgba {
        r: gamma(4.0767417 * l - 3.3077116 * m + 0.23096993 * s),
        g: gamma(-1.268438 * l + 2.6097574 * m - 0.3413194 * s),
        b: gamma(-0.0041960863 * l - 0.7034186 * m + 1.7076147 * s),
        a: 1.,
    }
}

pub fn add_glyph(colors: SetupColors) -> Div {
    div()
        .size(px(24.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(colors.border.opacity(0.6))
        .bg(crate::palette::rgb(crate::shell::BG))
        .flex()
        .items_center()
        .justify_center()
        .child(icon("plus").size(px(12.)).text_color(colors.muted))
}

pub fn pending<T: 'static>(
    name: &str,
    url: Option<&str>,
    busy: bool,
    colors: SetupColors,
    cx: &mut Context<T>,
    cancel: impl Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
) -> AnyElement {
    let url = url
        .filter(|url| url.starts_with("https://"))
        .map(str::to_owned);
    let button = |id, title| {
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(title)
            .tab_index(0)
            .h(px(28.))
            .px(px(10.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .text_size(px(12.))
            .cursor_pointer()
            .child(title)
            .hover(|d| d.bg(colors.border.opacity(0.4)))
    };
    div()
        .id("plugin-pending")
        .role(Role::Status)
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .rounded(px(8.))
        .bg(crate::settings_style::frame_bg().opacity(0.5))
        .py(px(6.))
        .pl(px(12.))
        .pr(px(6.))
        .text_size(px(13.))
        .text_color(colors.muted)
        .child(icon("refresh").size(px(14.)))
        .child(div().flex_1().min_w_0().child(format!(
            "Finish signing in to {name} in your browser. Zuse picks it up automatically."
        )))
        .when_some(url, |d, url| {
            d.child(
                button("authorize-hosted-plugin", "Reopen browser")
                    .on_click({
                        let url = url.clone();
                        move |_, _, cx| cx.open_url(&url)
                    })
                    .on_key_down(move |event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.open_url(&url);
                            cx.stop_propagation();
                        }
                    }),
            )
        })
        .child(
            button("cancel-plugin-attempt", "Cancel")
                .when(busy, |d| d.opacity(0.5))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !busy {
                        cancel(this, window, cx)
                    }
                }))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        if !busy {
                            cancel(this, window, cx)
                        }
                        cx.stop_propagation();
                    }
                })),
        )
        .into_any_element()
}
