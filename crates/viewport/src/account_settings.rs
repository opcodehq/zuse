//! Shared account presentation. Hosts own identity requests and preference storage.
use crate::{assets::icon, input::edit_menu, palette, settings_style as s};
use gpui::{prelude::*, *};
use gpui_base::input::{InputEditorStyle, InputEvent, InputState};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq)]
pub struct Colors {
    pub text: Hsla,
    pub muted: Hsla,
    pub background: Hsla,
    pub border: Hsla,
    pub danger: Hsla,
}
#[derive(Clone, PartialEq)]
pub struct Properties {
    pub status: Value,
    pub display_name: String,
    pub loaded: bool,
    pub available: bool,
    pub busy: bool,
    pub colors: Colors,
}
pub enum Event {
    SignIn,
    SignOut,
    DisplayName { account: String, value: String },
}
pub struct AccountCard {
    properties: Properties,
    name: Entity<InputState>,
    editing: Option<String>,
    edit_focus: FocusHandle,
    email_revealed: bool,
    clipboard_error: Option<&'static str>,
    menu: Option<edit_menu::EditMenu>,
    _input: Subscription,
}
impl EventEmitter<Event> for AccountCard {}
impl edit_menu::Host for AccountCard {
    fn edit_menu(&mut self) -> &mut Option<edit_menu::EditMenu> {
        &mut self.menu
    }
}
impl AccountCard {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let colors = Colors {
            text: palette::rgb(crate::shell::TEXT).into(),
            muted: palette::rgb(crate::shell::MUTED).into(),
            background: s::card_bg(),
            border: s::card_border(),
            danger: s::destructive_text(),
        };
        let owner = cx.weak_entity();
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Your name");
            input.on_context_menu(edit_menu::handler(owner, input.focus_handle(cx)));
            input
        });
        let input = cx.subscribe_in(&name, window, |this, input, event, window, cx| {
            match event {
                InputEvent::PressEnter { shift: false, .. } => this.finish(true, window, cx),
                InputEvent::Blur => this.finish(false, window, cx),
                InputEvent::ClipboardReadFailed => {
                    this.clipboard_error = Some(crate::input::CLIPBOARD_PASTE_ERROR)
                }
                InputEvent::ClipboardWriteFailed => {
                    this.clipboard_error = Some(crate::input::CLIPBOARD_COPY_ERROR)
                }
                InputEvent::Change | InputEvent::ClipboardWriteSucceeded => {
                    this.clipboard_error = None
                }
                _ => {}
            }
            input.update(cx, |_, cx| cx.notify());
            cx.notify();
        });
        Self {
            properties: Properties {
                status: Value::Null,
                display_name: String::new(),
                loaded: false,
                available: false,
                busy: false,
                colors,
            },
            name,
            editing: None,
            edit_focus: cx.focus_handle().tab_stop(true),
            email_revealed: false,
            clipboard_error: None,
            menu: None,
            _input: input,
        }
    }
    pub fn set_properties(&mut self, properties: Properties, cx: &mut Context<Self>) {
        if self.properties == properties {
            return;
        }
        if self.properties.status["user"]["id"] != properties.status["user"]["id"]
            || self.properties.display_name != properties.display_name
        {
            self.editing = None;
            self.email_revealed = false;
            self.clipboard_error = None;
            self.menu = None;
        }
        let c = properties.colors;
        self.name.update(cx, |input, cx| {
            input.set_editor_style(InputEditorStyle {
                foreground: c.text.into(),
                muted_foreground: c.muted.into(),
                background: c.background.into(),
                border: c.border.into(),
                caret: c.text.into(),
                ..Default::default()
            });
            cx.notify();
        });
        self.properties = properties;
        cx.notify();
    }
    fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.properties.status["user"]["id"].as_str() else {
            return;
        };
        self.editing = Some(owner.to_owned());
        self.clipboard_error = None;
        self.name.update(cx, |input, cx| {
            input.set_value(self.properties.display_name.clone(), window, cx)
        });
        window.focus(&self.name.read(cx).focus_handle(cx), cx);
        cx.notify();
    }
    fn finish(&mut self, return_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.editing.take() else {
            return;
        };
        if self.properties.status["user"]["id"] == owner {
            cx.emit(Event::DisplayName {
                account: owner,
                value: self.name.read(cx).value().to_string(),
            });
        }
        self.clipboard_error = None;
        self.menu = None;
        if return_focus {
            window.focus(&self.edit_focus, cx);
        }
        cx.notify();
    }
    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.clipboard_error = None;
        self.menu = None;
        window.focus(&self.edit_focus, cx);
        cx.notify();
    }
    fn email(&self, email: String, cx: &mut Context<Self>) -> AnyElement {
        let revealed = self.email_revealed;
        div()
            .id("general-account-email")
            .role(Role::Button)
            .aria_label(if revealed {
                "Hide email"
            } else {
                "Reveal email"
            })
            .tab_index(0)
            .relative()
            .cursor_pointer()
            .max_w(px(256.))
            .min_w_0()
            .rounded(px(4.))
            .bg(s::frame_bg().opacity(0.4))
            .px(px(4.))
            .py(px(2.))
            .font_family("Geist Mono")
            .text_size(px(11.))
            .line_height(px(14.85))
            .text_color(self.properties.colors.text)
            .child(div().truncate().child(email))
            .when(!revealed, |view| {
                view.child(
                    canvas(
                        |_, _, _| (),
                        |bounds, _, window, _| {
                            window.paint_layer(bounds, |window| {
                                window.paint_backdrop_blur(
                                    bounds,
                                    Corners::all(px(4.)),
                                    px(if bounds.contains(&window.mouse_position()) {
                                        3.
                                    } else {
                                        5.
                                    }),
                                )
                            });
                        },
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
            })
            .on_hover(cx.listener(|_, _, _, cx| cx.notify()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.email_revealed = !this.email_revealed;
                cx.notify();
            }))
            .into_any_element()
    }
}
impl Render for AccountCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.properties;
        let c = p.colors;
        let user = &p.status["user"];
        let signed_in = user.is_object();
        let waiting = p.status["login"].is_object();
        let busy = p.busy || waiting;
        let available = p.available;
        let p_busy = p.busy;
        let name = if !p.display_name.trim().is_empty() {
            p.display_name.trim().to_owned()
        } else {
            user["name"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| user["email"].as_str())
                .unwrap_or("Not signed in")
                .to_owned()
        };
        let email = user["email"].as_str().map(str::to_owned);
        let action = s::button(
            "general-account-sign-in",
            if signed_in {
                "Sign out"
            } else if busy {
                "Signing in…"
            } else {
                "Sign in"
            },
        )
        .tab_index(0)
        .when(busy || !available, |d| d.opacity(0.64))
        .on_click(cx.listener(move |_, _, _, cx| {
            if available && !busy {
                cx.emit(if signed_in {
                    Event::SignOut
                } else {
                    Event::SignIn
                });
            }
        }));
        let mut card = s::card();
        if signed_in {
            let mut profile = div().flex_1().min_w_0().flex().flex_col();
            if self.editing.is_some() {
                let focus = self.name.read(cx).focus_handle(cx);
                profile = profile.child(
                    div()
                        .id("general-display-name-field")
                        .debug_selector(|| "general-display-name-field".into())
                        .h(px(28.))
                        .w_full()
                        .max_w(px(220.))
                        .px(px(8.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(c.border.opacity(0.5))
                        .bg(palette::rgb(crate::shell::BG))
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .cursor_text()
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            window.focus(&focus, cx)
                        })
                        .on_action(
                            cx.listener(|this, _: &gpui_base::input::Escape, window, cx| {
                                this.cancel_edit(window, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                this.cancel_edit(window, cx);
                                cx.stop_propagation();
                            }
                        }))
                        .child(self.name.clone()),
                );
            } else {
                let email_name = email.as_ref().is_some_and(|email| email == &name);
                profile = profile.child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(px(6.))
                        .child(if email_name {
                            self.email(email.clone().unwrap_or_default(), cx)
                        } else {
                            div()
                                .truncate()
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(name.clone())
                                .into_any_element()
                        })
                        .child(
                            div()
                                .id("general-edit-display-name")
                                .role(Role::Button)
                                .aria_label("Edit display name")
                                .track_focus(&self.edit_focus)
                                .tab_index(0)
                                .cursor_pointer()
                                .flex_none()
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(7.))
                                .hover(|d| d.bg(s::frame_bg().opacity(0.6)))
                                .child(icon("pen").size(px(14.)).text_color(c.muted))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.start_edit(window, cx)),
                                ),
                        ),
                );
            }
            if let Some(email) = email.filter(|email| email != &name) {
                profile = profile.child(self.email(email, cx));
            }
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(14.))
                    .child(
                        div()
                            .size(px(40.))
                            .flex_none()
                            .rounded(px(10.))
                            .bg(s::frame_bg())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(
                                name.chars()
                                    .next()
                                    .unwrap_or('?')
                                    .to_uppercase()
                                    .to_string(),
                            ),
                    )
                    .child(profile)
                    .child(action),
            );
        } else {
            let (title, description) = if !available {
                (
                    "Account unavailable",
                    "Reconnect this computer to load the saved account session.",
                )
            } else if !p.loaded {
                (
                    "Checking account…",
                    "Loading the saved account session from this computer.",
                )
            } else {
                (
                    "Not signed in",
                    "You're using Zuse (Beta) locally without an account. Sign in to sync and unlock remote agents.",
                )
            };
            card = card.child(
                s::setting_row(true)
                    .child(s::details(title, description))
                    .when(available && p.loaded, |row| row.child(action)),
            );
        }
        if waiting {
            let code = p.status["login"]["userCode"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(c.border)
                    .child(s::description_text(format!(
                        "Enter {code} in your browser to finish signing in."
                    )))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .when_some(
                                p.status["login"]["url"]
                                    .as_str()
                                    .filter(|url| url.starts_with("https://"))
                                    .map(str::to_owned),
                                |d, url| {
                                    d.child(
                                        s::button(
                                            "general-authorize-account",
                                            "Continue in browser",
                                        )
                                        .tab_index(0)
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                    )
                                },
                            )
                            .child(
                                s::button("general-cancel-sign-in", "Cancel")
                                    .tab_index(0)
                                    .when(p.busy, |d| d.opacity(0.64))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        if !p_busy && available {
                                            cx.emit(Event::SignOut);
                                        }
                                    })),
                            ),
                    ),
            );
        }
        if let Some(error) = p.status["error"].as_str().filter(|error| !error.is_empty()) {
            card = card.child(
                div()
                    .id("general-account-error")
                    .role(Role::Alert)
                    .px(px(12.))
                    .py(px(10.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(c.danger)
                    .child(error.to_owned()),
            );
        }
        if let Some(error) = self.clipboard_error {
            card = card.child(
                div()
                    .id("general-display-name-error")
                    .role(Role::Alert)
                    .px(px(12.))
                    .py(px(10.))
                    .text_size(px(11.))
                    .text_color(c.danger)
                    .child(error),
            );
        }
        s::frame().text_color(c.text).child(s::header("Account","Sign in to sync your account across devices and (soon) drive remote agents from your phone."))
            .child(card).when_some(self.menu.as_ref(),|view,menu|view.child(edit_menu::render(menu,cx)))
    }
}
