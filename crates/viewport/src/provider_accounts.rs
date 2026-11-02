//! Provider account rows and their shared switch/remove interactions.
use crate::localization::message as tr;
use crate::{
    acp_catalog::popup_bg,
    assets::icon,
    palette::rgb,
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use std::{cell::Cell, collections::HashMap, rc::Rc};
use zuse_i18n::messages as msg;
use zuse_protocol::AgentAccount;

#[derive(Clone, Default, PartialEq)]
pub struct Data {
    pub scope: String,
    pub accounts: Vec<AgentAccount>,
    pub blocked: bool,
}
#[derive(Clone)]
pub enum Action {
    Switch(String),
    Remove(String),
}
pub struct Event {
    pub scope: String,
    pub action: Action,
}
actions!(
    provider_account_menu,
    [Previous, Next, First, Last, Choose, Close]
);
struct Bindings;
impl Global for Bindings {}
struct Trigger {
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
fn dialog_button(
    id: &'static str,
    label: &'static str,
    destructive: bool,
    pending: bool,
) -> Stateful<Div> {
    let color = s::destructive_text();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .on_key_down(|event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
            }
        })
        .cursor_pointer()
        .relative()
        .h(px(28.))
        .px(px(15.))
        .border_1()
        .border_color(if destructive {
            color
        } else {
            transparent_black()
        })
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .bg(if destructive {
            color
        } else {
            transparent_black()
        })
        .text_color(if destructive {
            white()
        } else {
            rgb(TEXT).into()
        })
        .hover(move |v| {
            v.bg(if destructive {
                color.opacity(0.9)
            } else {
                s::frame_bg()
            })
        })
        .when(pending, |v| v.opacity(0.64).cursor_default())
        .child(
            div()
                .when(pending && destructive, |v| v.opacity(0.))
                .child(label),
        )
        .when(pending && destructive, |v| {
            v.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::acp_catalog::spinner("account-removal-spinner", 12.)),
            )
        })
}
pub struct AccountRows {
    data: Data,
    triggers: HashMap<String, Trigger>,
    menu: Option<(String, usize)>,
    removing: Option<AgentAccount>,
    pending: bool,
    error: Option<String>,
    usage: bool,
    focus: FocusHandle,
    list_focus: FocusHandle,
    dialog_focus: FocusHandle,
    usage_focus: FocusHandle,
    cancel: FocusHandle,
    confirm: FocusHandle,
    restore: Option<String>,
    retry_focus: bool,
    focus_epoch: u64,
}
impl EventEmitter<Event> for AccountRows {}
impl AccountRows {
    pub fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<Bindings>() {
            cx.set_global(Bindings);
            cx.bind_keys([
                KeyBinding::new("up", Previous, Some("ProviderAccountMenu")),
                KeyBinding::new("down", Next, Some("ProviderAccountMenu")),
                KeyBinding::new("home", First, Some("ProviderAccountMenu")),
                KeyBinding::new("end", Last, Some("ProviderAccountMenu")),
                KeyBinding::new("enter", Choose, Some("ProviderAccountMenu")),
                KeyBinding::new("space", Choose, Some("ProviderAccountMenu")),
                KeyBinding::new("escape", Close, Some("ProviderAccountMenu")),
                KeyBinding::new("tab", Close, Some("ProviderAccountMenu")),
                KeyBinding::new("shift-tab", Close, Some("ProviderAccountMenu")),
                KeyBinding::new("escape", Close, Some("ProviderAccountDialog")),
                KeyBinding::new("tab", Next, Some("ProviderAccountDialog")),
                KeyBinding::new("shift-tab", Previous, Some("ProviderAccountDialog")),
                KeyBinding::new("enter", Choose, Some("ProviderAccountDialog")),
                KeyBinding::new("space", Choose, Some("ProviderAccountDialog")),
            ]);
        }
        Self {
            data: Data::default(),
            triggers: HashMap::new(),
            menu: None,
            removing: None,
            pending: false,
            error: None,
            usage: false,
            focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            dialog_focus: cx.focus_handle(),
            usage_focus: cx.focus_handle().tab_stop(true),
            cancel: cx.focus_handle().tab_stop(true),
            confirm: cx.focus_handle().tab_stop(true),
            restore: None,
            retry_focus: false,
            focus_epoch: 0,
        }
    }
    fn focus_after_mount(&self, focus: FocusHandle, window: &mut Window, cx: &Context<Self>) {
        let owner = cx.weak_entity();
        let epoch = self.focus_epoch;
        window.on_next_frame(move |window, cx| {
            let _ = owner.update(cx, |this, cx| {
                if this.focus_epoch == epoch {
                    window.focus(&focus, cx);
                }
            });
        });
    }
    pub fn sync(&mut self, data: Data, cx: &mut Context<Self>) {
        if self.data == data {
            return;
        }
        if self.data.scope != data.scope {
            self.focus_epoch += 1;
            self.menu = None;
            self.removing = None;
            self.pending = false;
            self.error = None;
            self.usage = false;
            self.restore = None;
            self.retry_focus = false;
        }
        if data.blocked
            || self
                .menu
                .as_ref()
                .is_some_and(|(id, _)| !data.accounts.iter().any(|a| &a.id == id))
        {
            self.menu = None;
        }
        self.data = data;
        cx.notify();
    }
    pub fn finish(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.focus_epoch += 1;
        self.pending = false;
        match result {
            Ok(()) => {
                self.restore = self.removing.take().map(|a| a.id);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                self.retry_focus = self.removing.is_some();
            }
        }
        cx.notify();
    }
    fn close_menu(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.menu.take() {
            self.focus_epoch += 1;
            if let Some(trigger) = self.triggers.get(&id) {
                let focus = trigger.focus.clone();
                if restore {
                    window.focus(&focus, cx);
                } else {
                    let owner = cx.weak_entity();
                    let handle = window.window_handle();
                    cx.defer(move |cx| {
                        let _ = cx.update_window(handle, |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                if this.menu.is_none() && this.focus.is_focused(window) {
                                    window.focus(&focus, cx);
                                }
                            });
                        });
                    });
                }
            }
        }
        cx.notify();
    }
    fn dismiss_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        if let Some(account) = self.removing.take() {
            self.focus_epoch += 1;
            if let Some(trigger) = self.triggers.get(&account.id) {
                window.focus(&trigger.focus, cx);
            }
        }
        self.error = None;
        cx.notify();
    }
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, index)) = self.menu.clone() else {
            return;
        };
        let Some(account) = self.data.accounts.iter().find(|a| a.id == id).cloned() else {
            return;
        };
        if self.pending || self.data.blocked || !account.switchable {
            return;
        }
        self.close_menu(true, window, cx);
        self.error = None;
        if index == 0 && !account.active {
            self.pending = true;
            cx.emit(Event {
                scope: self.data.scope.clone(),
                action: Action::Switch(id),
            });
        } else if index == 1 {
            self.removing = Some(account);
            window.focus(&self.cancel, cx);
            self.focus_after_mount(self.cancel.clone(), window, cx);
        }
        cx.notify();
    }
    fn remove(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.data.blocked {
            return;
        }
        if let Some(account) = &self.removing {
            self.focus_epoch += 1;
            self.pending = true;
            self.error = None;
            cx.emit(Event {
                scope: self.data.scope.clone(),
                action: Action::Remove(account.id.clone()),
            });
            cx.notify();
        }
    }
    fn menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, selected) = self.menu.as_ref()?;
        let account = self.data.accounts.iter().find(|a| &a.id == id)?;
        let bounds = self.triggers.get(id)?.bounds.get();
        let (anchor, at) = if bounds.bottom() + px(81.) < window.viewport_size().height {
            (
                Anchor::TopRight,
                point(bounds.right(), bounds.bottom() + px(4.)),
            )
        } else {
            (
                Anchor::BottomRight,
                point(bounds.right(), bounds.top() - px(4.)),
            )
        };
        let active = account.active;
        let label = "Switch to this account";
        let mut font = window.text_style().font();
        font.weight = FontWeight::NORMAL;
        let width = f32::from(
            window
                .text_system()
                .shape_line(
                    label.into(),
                    px(12.),
                    &[TextRun {
                        len: label.len(),
                        font,
                        color: window.text_style().color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
                .width(),
        ) + 22.;
        let popup = div()
            .id("provider-account-menu")
            .role(Role::Menu)
            .aria_label(tr(cx, msg::native::account_actions))
            .key_context("ProviderAccountMenu")
            .track_focus(&self.focus)
            .w(px(width.max(112.)))
            .p(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(s::card_border())
            .bg(popup_bg())
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if !bounds.contains(&event.position) {
                        this.close_menu(false, window, cx);
                    }
                }),
            )
            .on_action(cx.listener(move |this, _: &Previous, _, cx| {
                if let Some((_, i)) = &mut this.menu {
                    *i = if active { 1 } else { 1 - *i };
                }
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &Next, _, cx| {
                if let Some((_, i)) = &mut this.menu {
                    *i = if active { 1 } else { 1 - *i };
                }
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &First, _, cx| {
                if let Some((_, i)) = &mut this.menu {
                    *i = usize::from(active);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Last, _, cx| {
                if let Some((_, i)) = &mut this.menu {
                    *i = 1;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Choose, window, cx| this.choose(window, cx)))
            .on_action(cx.listener(|this, _: &Close, window, cx| this.close_menu(true, window, cx)))
            .children(
                ["Switch to this account", "Remove"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| {
                        div()
                            .when(index == 1, |v| {
                                v.child(div().h(px(1.)).my(px(4.)).bg(s::card_border()))
                            })
                            .child(
                                div()
                                    .id(("provider-account-action", index))
                                    .role(Role::MenuItem)
                                    .aria_label(label)
                                    .h(px(24.))
                                    .px(px(6.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.))
                                    .child(label)
                                    .text_color(if index == 1 {
                                        s::destructive_text()
                                    } else {
                                        rgb(TEXT).into()
                                    })
                                    .when(index == 0 && active, |v| v.opacity(0.5))
                                    .when(index == 1 || !active, |v| {
                                        v.cursor_pointer()
                                            .when(*selected == index, |v| v.bg(s::frame_bg()))
                                            .hover(|v| v.bg(s::frame_bg()))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if let Some((_, i)) = &mut this.menu {
                                                    *i = index;
                                                }
                                                this.choose(window, cx);
                                            }))
                                    }),
                            )
                    }),
            );
        Some(
            deferred(anchored().anchor(anchor).position(at).child(popup))
                .priority(4)
                .into_any_element(),
        )
    }
    fn dialog(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let account = self.removing.as_ref()?;
        let title = format!("Remove {}?", label(account));
        let description = if account.active {
            "Removes this saved account and signs the CLI out of this login. Chats may need you to sign in again. Transcripts are kept."
        } else {
            "Removes this saved login from Settings. Sign in again to use this account. The active CLI login and transcripts are kept."
        };
        let card = div()
            .id("provider-account-remove-dialog")
            .role(Role::AlertDialog)
            .aria_label(title.clone())
            .track_focus(&self.dialog_focus)
            .key_context("ProviderAccountDialog")
            .tab_group()
            .tab_stop(false)
            .on_action(cx.listener(|this, _: &Close, window, cx| this.dismiss_dialog(window, cx)))
            .on_action(cx.listener(|this, _: &Next, window, cx| {
                this.focus_epoch += 1;
                window.focus(
                    if this.cancel.is_focused(window) {
                        &this.confirm
                    } else {
                        &this.cancel
                    },
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &Previous, window, cx| {
                this.focus_epoch += 1;
                window.focus(
                    if this.cancel.is_focused(window) {
                        &this.confirm
                    } else {
                        &this.cancel
                    },
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &Choose, window, cx| {
                if this.confirm.is_focused(window) {
                    this.remove(cx)
                } else {
                    this.dismiss_dialog(window, cx)
                }
            }))
            .flex_none()
            .w(px(384.))
            .max_w(window.viewport_size().width - px(32.))
            .max_h(window.viewport_size().height - px(32.))
            .overflow_y_scroll()
            .rounded(px(12.))
            .border_1()
            .border_color(s::card_border())
            .bg(popup_bg())
            .text_color(rgb(TEXT))
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(15.))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(20.))
                            .text_color(rgb(MUTED))
                            .child(description),
                    ),
            )
            .when_some(self.error.clone(), |v, error| {
                v.child(
                    div()
                        .id("provider-account-action-error")
                        .role(Role::Alert)
                        .px(px(16.))
                        .pb(px(10.))
                        .text_size(px(12.))
                        .text_color(s::destructive_text())
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(s::card_border())
                    .bg(s::frame_bg())
                    .rounded_b(px(11.))
                    .child(
                        dialog_button(
                            "cancel-provider-account-remove",
                            "Cancel",
                            false,
                            self.pending,
                        )
                        .track_focus(&self.cancel)
                        .tab_index(0)
                        .when(self.pending, |v| v.opacity(0.5))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.dismiss_dialog(window, cx)),
                        ),
                    )
                    .child(
                        dialog_button(
                            "confirm-provider-account-remove",
                            "Remove",
                            true,
                            self.pending,
                        )
                        .track_focus(&self.confirm)
                        .tab_index(0)
                        .when(self.pending, |v| v.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                    ),
            );
        let overlay = div()
            .w(window.viewport_size().width)
            .h(window.viewport_size().height)
            .bg(rgba(0x00000052))
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .p(px(16.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.dismiss_dialog(window, cx);
                    cx.stop_propagation();
                }),
            )
            .child(div().flex_1())
            .child(card)
            .child(div().flex_1().flex_grow(3.));
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(point(px(0.), px(0.)))
                    .child(overlay),
            )
            .priority(5)
            .into_any_element(),
        )
    }
}
fn label(account: &AgentAccount) -> String {
    account
        .display_name
        .clone()
        .filter(|name| !name.trim().is_empty() && Some(name) != account.email.as_ref())
        .or_else(|| {
            account
                .email
                .as_deref()
                .map(zuse_protocol::presentation::masked_email)
        })
        .unwrap_or_else(|| "Unknown account".into())
}
impl Render for AccountRows {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.retry_focus) && self.removing.is_some() {
            window.focus(&self.confirm, cx);
            self.focus_after_mount(self.confirm.clone(), window, cx);
        }
        if let Some(id) = self.restore.take() {
            window.focus(
                &self
                    .triggers
                    .get(&id)
                    .filter(|_| self.data.accounts.iter().any(|a| a.id == id))
                    .map(|t| t.focus.clone())
                    .unwrap_or_else(|| self.list_focus.clone()),
                cx,
            );
        }
        let menu = self.menu(window, cx);
        let dialog = self.dialog(window, cx);
        let blocked = self.data.blocked || self.pending;
        let rows = self
            .data
            .accounts
            .iter()
            .enumerate()
            .map(|(index, account)| {
                let id = account.id.clone();
                let name = label(account);
                let raw = account
                    .display_name
                    .as_deref()
                    .or(account.email.as_deref())
                    .unwrap_or("?");
                let initials = raw
                    .split_whitespace()
                    .take(2)
                    .filter_map(|s| s.chars().next())
                    .collect::<String>()
                    .to_uppercase();
                let colors = [
                    (0x0ea5e9, 0x7dd3fc),
                    (0x8b5cf6, 0xc4b5fd),
                    (0x10b981, 0x6ee7b7),
                    (0xf59e0b, 0xfcd34d),
                    (0xf43f5e, 0xfda4af),
                    (0xd946ef, 0xf0abfc),
                ];
                let tone = id.encode_utf16().map(usize::from).sum::<usize>() % colors.len();
                let (bg, fg) = colors[tone];
                let trigger = self.triggers.entry(id.clone()).or_insert_with(|| Trigger {
                    focus: cx.focus_handle().tab_stop(true),
                    bounds: Rc::new(Cell::new(Bounds::default())),
                });
                let focus = trigger.focus.clone();
                let bounds = trigger.bounds.clone();
                let open = self
                    .menu
                    .as_ref()
                    .is_some_and(|(current, _)| current == &id);
                div()
                    .id(("provider-account-row", index))
                    .min_h(px(36.))
                    .px(px(10.))
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(20.))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(bg).opacity(0.2))
                            .text_color(rgb(fg))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(9.))
                            .line_height(px(12.15))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(initials),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(rgb(TEXT))
                                    .child(name.clone()),
                            )
                            .when(account.active, |v| {
                                v.child(
                                    div()
                                        .flex_none()
                                        .rounded_full()
                                        .px(px(6.))
                                        .py(px(1.))
                                        .bg(rgb(TEXT).opacity(0.08))
                                        .text_color(rgb(TEXT).opacity(0.8))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_size(px(10.))
                                        .line_height(px(13.5))
                                        .child(tr(cx, msg::chat::create_from_menu_in_use)),
                                )
                            })
                            .when_some(account.plan_label.clone(), |v, plan| {
                                v.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(11.))
                                        .line_height(px(14.85))
                                        .text_color(rgb(MUTED))
                                        .child(plan),
                                )
                            }),
                    )
                    .when(account.switchable, |v| {
                        v.child(
                            s::button(("provider-account-more", index), "")
                                .size(px(24.))
                                .px_0()
                                .border_0()
                                .bg(transparent_black())
                                .relative()
                                .track_focus(&focus)
                                .tab_index(0)
                                .aria_label(format!("Actions for {name}"))
                                .opacity(if open { 1. } else { 0.6 })
                                .child(icon("more-horizontal").size(px(14.)))
                                .when(blocked, |v| v.cursor_default().opacity(0.4))
                                .child(
                                    canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {})
                                        .absolute()
                                        .size_full(),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.pending || this.data.blocked {
                                        return;
                                    }
                                    if this
                                        .menu
                                        .as_ref()
                                        .is_some_and(|(current, _)| current == &id)
                                    {
                                        this.close_menu(true, window, cx);
                                        return;
                                    }
                                    let active = this
                                        .data
                                        .accounts
                                        .iter()
                                        .find(|a| a.id == id)
                                        .is_some_and(|a| a.active);
                                    this.menu = Some((id.clone(), usize::from(active)));
                                    this.focus_epoch += 1;
                                    this.error = None;
                                    window.focus(&this.focus, cx);
                                    this.focus_after_mount(this.focus.clone(), window, cx);
                                    cx.notify();
                                })),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let mut body = div()
            .id("provider-accounts-list")
            .track_focus(&self.list_focus)
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .line_height(px(16.))
            .children(rows)
            .when_some(
                self.error.clone().filter(|_| self.removing.is_none()),
                |v, error| {
                    v.child(
                        div()
                            .id("provider-account-action-error")
                            .role(Role::Alert)
                            .px(px(10.))
                            .text_size(px(11.))
                            .text_color(s::destructive_text())
                            .child(error),
                    )
                },
            );
        if !self.data.accounts.is_empty() {
            body = body.child(
                s::button(
                    "provider-account-usage",
                    if self.usage {
                        "Hide usage details"
                    } else {
                        "Usage details"
                    },
                )
                .self_start()
                .track_focus(&self.usage_focus)
                .tab_index(0)
                .text_size(px(11.))
                .border_0()
                .bg(transparent_black())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.usage = !this.usage;
                    cx.notify();
                })),
            );
            if self.usage {
                for (index, account) in self.data.accounts.iter().enumerate() {
                    let mut details = div()
                        .id(("provider-account-usage-detail", index))
                        .px(px(10.))
                        .py(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(div().text_color(rgb(TEXT)).child(label(account)));
                    for usage in &account.usage_windows {
                        let fraction = usage.used_fraction.clamp(0., 1.);
                        let color = if fraction >= 0.95 {
                            s::destructive_text()
                        } else if fraction >= 0.8 {
                            rgb(0xf59e0b).into()
                        } else {
                            rgb(0x89ca21).into()
                        };
                        details = details.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(format!("{} · {:.0}% used", usage.label, fraction * 100.))
                                .child(
                                    div()
                                        .w(px(88.))
                                        .h(px(4.))
                                        .rounded_full()
                                        .bg(s::frame_bg())
                                        .child(
                                            div()
                                                .w(relative(fraction))
                                                .h_full()
                                                .rounded_full()
                                                .bg(color),
                                        ),
                                )
                                .when_some(usage.resets_at, |v, reset| {
                                    v.child(format!("Resets {}", reset.format("%b %-d, %H:%M UTC")))
                                }),
                        );
                    }
                    if let Some(error) = &account.usage_error {
                        details = details.child(error.clone());
                    } else if account.usage_windows.is_empty() {
                        details =
                            details.child(tr(cx, msg::native::usage_not_reported_for_this_account));
                    }
                    body = body.child(details);
                }
            }
        }
        body.children(menu).children(dialog)
    }
}
