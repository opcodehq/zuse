//! Shared ephemeral plugin setup form for both GPUI frontends.
use crate::localization::message as tr;
use gpui::{prelude::*, *};
use gpui_base::input::{InputEditorStyle, InputEvent, InputState, TextareaState};
use zuse_i18n::messages as msg;
use zuse_protocol::{
    plugin_setup::{LOCAL_CONFIG_TEMPLATE, LocalServerDraft, ServerAuth, ServerDraft, api_key},
    plugins::{PluginDefinition, PluginRequest, PluginReturnTo},
};

pub enum SetupTarget {
    Catalog(PluginDefinition),
    Server,
}
pub enum SetupEvent {
    Submit(PluginRequest),
    Cancel,
}

#[derive(Clone, Copy, PartialEq)]
pub struct SetupColors {
    pub text: Hsla,
    pub muted: Hsla,
    pub background: Hsla,
    pub border: Hsla,
    pub danger: Hsla,
}

pub struct PluginSetup {
    tenant: String,
    target: SetupTarget,
    return_to: PluginReturnTo,
    name: Entity<InputState>,
    dialog_focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    focus_on_mount: bool,
    focus_at_open: Option<FocusHandle>,
    cancel_focus: FocusHandle,
    submit_focus: FocusHandle,
    auth_focus: [FocusHandle; 3],
    url: Entity<InputState>,
    header: Entity<InputState>,
    secret: Entity<InputState>,
    auth: ServerAuth,
    /// Personal workspaces may run a stdio server on this computer instead of a URL.
    local: bool,
    transport_focus: [FocusHandle; 2],
    /// One pasted stdio server configuration in JSON.
    config: Entity<TextareaState>,
    error: Option<&'static str>,
    menu: Option<crate::input::edit_menu::EditMenu>,
    pub busy: bool,
    pub colors: SetupColors,
    _inputs: Vec<Subscription>,
}
impl EventEmitter<SetupEvent> for PluginSetup {}
impl crate::input::edit_menu::Host for PluginSetup {
    fn edit_menu(&mut self) -> &mut Option<crate::input::edit_menu::EditMenu> {
        &mut self.menu
    }
}
impl PluginSetup {
    pub fn new(
        tenant: String,
        target: SetupTarget,
        return_to: PluginReturnTo,
        colors: SetupColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let style = InputEditorStyle {
            foreground: colors.text.into(),
            muted_foreground: colors.muted.into(),
            background: colors.background.into(),
            border: colors.border.into(),
            caret: colors.text.into(),
            ..Default::default()
        };
        let owner = cx.weak_entity();
        let mut field = |placeholder: &str, masked: bool| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx)
                    .placeholder(placeholder.to_owned())
                    .masked(masked);
                input.set_editor_style(style.clone());
                input.on_context_menu(crate::input::edit_menu::handler(
                    owner.clone(),
                    input.focus_handle(cx),
                ));
                input
            })
        };
        let name = field("Internal tools", false);
        let url = field("https://mcp.example.com/mcp", false);
        let header = field("X-API-Key", false);
        let secret = field(
            if matches!(target, SetupTarget::Catalog(_)) {
                "Key for this account"
            } else {
                "API key"
            },
            true,
        );
        let config = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx)
                .auto_grow(6, 14)
                .submit_on_enter(false);
            input.set_editor_style(style.clone());
            input.on_context_menu(crate::input::edit_menu::handler(
                owner.clone(),
                input.focus_handle(cx),
            ));
            input.set_value(LOCAL_CONFIG_TEMPLATE, window, cx);
            input
        });
        let mut subscriptions: Vec<Subscription> = [&name, &url, &header, &secret]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, input, event: &InputEvent, cx| {
                    match event {
                        InputEvent::PressEnter { shift: false, .. } => {
                            this.submit(cx);
                            return;
                        }
                        InputEvent::ClipboardReadFailed => {
                            this.error = Some(crate::input::CLIPBOARD_PASTE_ERROR);
                        }
                        InputEvent::ClipboardWriteFailed => {
                            this.error = Some(crate::input::CLIPBOARD_COPY_ERROR);
                        }
                        InputEvent::Change => this.error = None,
                        InputEvent::ClipboardWriteSucceeded
                            if matches!(
                                this.error,
                                Some(
                                    crate::input::CLIPBOARD_PASTE_ERROR
                                        | crate::input::CLIPBOARD_COPY_ERROR
                                )
                            ) =>
                        {
                            this.error = None;
                        }
                        _ => {}
                    }
                    // Focus changes must repaint the editor's platform text handler.
                    input.update(cx, |_, cx| cx.notify());
                    cx.notify();
                })
            })
            .collect();
        subscriptions.push(
            cx.subscribe(&config, |this, input, event: &InputEvent, cx| {
                match event {
                    InputEvent::Change => this.error = None,
                    InputEvent::ClipboardReadFailed => {
                        this.error = Some(crate::input::CLIPBOARD_PASTE_ERROR);
                    }
                    InputEvent::ClipboardWriteFailed => {
                        this.error = Some(crate::input::CLIPBOARD_COPY_ERROR);
                    }
                    _ => {}
                }
                input.update(cx, |_, cx| cx.notify());
                cx.notify();
            }),
        );
        if let SetupTarget::Catalog(plugin) = &target {
            name.update(cx, |input, cx| {
                input.set_value(plugin.name.clone(), window, cx)
            });
        }
        let return_focus = window.focused(cx);
        Self {
            tenant,
            target,
            return_to,
            name,
            dialog_focus: cx.focus_handle(),
            focus_at_open: return_focus.clone(),
            return_focus,
            focus_on_mount: true,
            cancel_focus: cx.focus_handle().tab_stop(true),
            submit_focus: cx.focus_handle().tab_stop(true),
            auth_focus: std::array::from_fn(|_| cx.focus_handle()),
            url,
            header,
            secret,
            auth: ServerAuth::Automatic,
            local: false,
            transport_focus: std::array::from_fn(|_| cx.focus_handle()),
            config,
            error: None,
            menu: None,
            busy: false,
            colors,
            _inputs: subscriptions,
        }
    }
    pub fn set_connection_label(
        &mut self,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.name
            .update(cx, |input, cx| input.set_value(label, window, cx));
    }
    pub fn is_inline(&self) -> bool {
        matches!(self.target, SetupTarget::Catalog(_))
    }
    pub fn set_return_focus(&mut self, focus: Option<FocusHandle>) {
        if focus.is_some() {
            self.return_focus = focus;
        }
    }
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(focus) = self.return_focus.clone() {
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        cx.emit(SetupEvent::Cancel);
    }
    fn inline(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.secret.read(cx).focus_handle(cx);
        let enabled = !self.busy && !self.secret.read(cx).value().trim().is_empty();
        div()
            .id("plugin-inline-key")
            .flex()
            .flex_col()
            .track_focus(&self.dialog_focus)
            .tab_group()
            .tab_stop(false)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.cancel(window, cx),
                    "tab" => {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                        if !this.dialog_focus.contains_focused(window, cx) {
                            let focus = if event.keystroke.modifiers.shift {
                                this.submit_focus.clone()
                            } else {
                                this.secret.read(cx).focus_handle(cx)
                            };
                            window.focus(&focus, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(8.))
                    .child(crate::plugin_detail::add_glyph(self.colors))
                    .child(
                        div()
                            .id("API key")
                            .debug_selector(|| "API key".into())
                            .flex_1()
                            .min_w_0()
                            .h(px(28.))
                            .relative()
                            .when(focus.is_focused(window), |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .inset(px(-2.))
                                        .rounded(px(8.))
                                        .border_2()
                                        .border_color(crate::palette::rgb(0x89ca21).opacity(0.24)),
                                )
                            })
                            .text_size(px(13.))
                            .when(!self.secret.read(cx).value().is_empty(), |d| {
                                d.font_family("Geist Mono")
                            })
                            .rounded(px(6.))
                            .bg(crate::palette::rgb(crate::shell::BG).opacity(0.7))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .cursor_text()
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                window.focus(&focus, cx)
                            })
                            .child(self.secret.clone()),
                    )
                    .child(
                        div()
                            .id("cancel-plugin-setup")
                            .role(Role::Button)
                            .aria_label(tr(cx, msg::common::cancel))
                            .track_focus(&self.cancel_focus)
                            .tab_index(0)
                            .h(px(28.))
                            .px(px(15.))
                            .border_1()
                            .border_color(transparent_black())
                            .rounded(px(7.))
                            .flex()
                            .items_center()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .child(tr(cx, msg::common::cancel))
                            .hover(|d| d.bg(self.colors.border.opacity(0.4)))
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.cancel(window, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    )
                    .child(
                        crate::styling::dither_action(
                            "submit-plugin-setup",
                            "Connect",
                            enabled,
                            true,
                        )
                        .px(px(15.))
                        .text_size(px(12.))
                        .track_focus(&self.submit_focus)
                        .tab_index(0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.submit(cx)
                            }
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    if enabled {
                                        this.submit(cx)
                                    }
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
            )
            .when_some(self.error, |d, error| {
                d.child(
                    div()
                        .id("plugin-form-error")
                        .role(Role::Alert)
                        .text_color(self.colors.danger)
                        .pb(px(8.))
                        .child(error),
                )
            })
            .when_some(self.menu.as_ref(), |d, menu| {
                d.child(crate::input::edit_menu::render(menu, cx))
            })
            .into_any_element()
    }
    /// Local servers belong to Personal; organization workspaces only offer Remote.
    fn offers_local(&self) -> bool {
        matches!(self.target, SetupTarget::Server) && self.tenant.starts_with("personal:")
    }
    fn set_local(&mut self, local: bool, cx: &mut Context<Self>) {
        if !self.busy && self.offers_local() {
            self.local = local;
            self.error = None;
            cx.notify();
        }
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().to_string();
        let secret = self.secret.read(cx).value().to_string();
        let result = match &self.target {
            SetupTarget::Server if self.local => LocalServerDraft {
                name,
                config: self.config.read(cx).value().to_string(),
            }
            .request(&self.tenant),
            SetupTarget::Server => ServerDraft {
                name,
                url: self.url.read(cx).value().to_string(),
                auth: self.auth,
                header: self.header.read(cx).value().to_string(),
                secret,
            }
            .request(&self.tenant, self.return_to.clone()),
            SetupTarget::Catalog(plugin) => {
                let name = name.trim();
                if name.is_empty() || name.chars().count() > 60 {
                    Err("Enter a connection name between 1 and 60 characters.")
                } else {
                    plugin
                        .api_key_header
                        .as_ref()
                        .map(|header| api_key(header, &secret).map(|key| key.value))
                        .transpose()
                        .map(|api_key| PluginRequest::Connect {
                            tenant_id: self.tenant.clone(),
                            plugin_id: plugin.id.clone(),
                            label: name.into(),
                            request_id: uuid::Uuid::new_v4().to_string(),
                            return_to: Some(self.return_to.clone()),
                            api_key,
                        })
                }
            }
        };
        match result {
            Ok(request) => {
                self.error = None;
                cx.emit(SetupEvent::Submit(request));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}
impl Render for PluginSetup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_on_mount {
            self.focus_on_mount = false;
            let current = window.focused(cx);
            if !self.busy && (current.is_none() || current == self.focus_at_open) {
                let field = if self.is_inline() {
                    &self.secret
                } else {
                    &self.name
                };
                window.focus(&field.read(cx).focus_handle(cx), cx);
                field.update(cx, |_, cx| cx.notify());
            }
        }
        if self.is_inline() {
            return self.inline(window, cx);
        }
        let viewport = window.viewport_size();
        let server = matches!(self.target, SetupTarget::Server);
        let local = server && self.local;
        let needs_key = match &self.target {
            SetupTarget::Catalog(p) => p.api_key_header.is_some(),
            SetupTarget::Server => !local && self.auth != ServerAuth::Automatic,
        };
        let field = |title: &'static str, input: Entity<InputState>| {
            let focus = input.read(cx).focus_handle(cx);
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .id(title)
                        .debug_selector(|| title.to_owned())
                        .cursor_text()
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            window.focus(&focus, cx)
                        })
                        .border_1()
                        .border_color(self.colors.border)
                        .rounded(px(7.))
                        .h(px(28.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .child(input),
                )
        };
        let area = |title: &'static str, input: Entity<TextareaState>| {
            let focus = input.read(cx).focus_handle(cx);
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .id(title)
                        .debug_selector(|| title.to_owned())
                        .cursor_text()
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            window.focus(&focus, cx)
                        })
                        .border_1()
                        .border_color(self.colors.border)
                        .rounded(px(7.))
                        .px(px(9.))
                        .py(px(5.))
                        .font_family("Geist Mono")
                        .child(input),
                )
        };
        let button = |id: &'static str, title: &'static str| {
            div()
                .id(id)
                .tab_index(0)
                .role(Role::Button)
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.))
                .px(px(15.))
                .rounded(px(7.))
                .text_size(px(12.))
                .child(title)
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(16.))
            .pt(px(4.))
            .pb(px(16.));
        if self.offers_local() {
            body = body.child(
                div().flex().flex_col().child(
                    div()
                        .id("plugin-transport-tabs")
                        .role(Role::TabList)
                        .aria_label(tr(cx, msg::plugins::plugins_transport))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .p(px(2.))
                        .gap(px(2.))
                        .rounded(px(7.))
                        .bg(crate::settings_style::frame_bg().opacity(0.55))
                        .children(
                            [
                                (false, "Remote", "Remote"),
                                (true, "Local", "Local command"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(i, (local, key, title))| {
                                let handles = self.transport_focus.clone();
                                let selected = self.local == local;
                                div()
                                    .id(("plugin-transport", i))
                                    .debug_selector(move || format!("plugin-transport-{key}"))
                                    .role(Role::Tab)
                                    .aria_label(title)
                                    .aria_selected(selected)
                                    .track_focus(
                                        &self.transport_focus[i].clone().tab_stop(selected),
                                    )
                                    .tab_index(if selected { 0 } else { -1 })
                                    .cursor_pointer()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px(px(10.))
                                    .rounded(px(5.))
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if selected {
                                        self.colors.text
                                    } else {
                                        self.colors.muted
                                    })
                                    .when(selected, |d| d.bg(crate::palette::rgb(crate::shell::BG)))
                                    .child(title)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.set_local(local, cx)
                                        }),
                                    )
                                    .on_key_down(cx.listener(
                                        move |this, event: &KeyDownEvent, window, cx| {
                                            let index = match event.keystroke.key.as_str() {
                                                "right" | "left" => 1 - i,
                                                "home" => 0,
                                                "end" => 1,
                                                _ => return,
                                            };
                                            if !this.busy {
                                                this.set_local(index == 1, cx);
                                                window.focus(&handles[index], cx);
                                            }
                                            cx.stop_propagation();
                                        },
                                    ))
                            }),
                        ),
                ),
            );
        }
        body = body.child(field("Name", self.name.clone()));
        if local {
            body = body.child(
                area("MCP configuration (JSON)", self.config.clone()).child(
                    div()
                        .text_color(self.colors.muted)
                        .child(tr(cx, msg::plugins::plugins_local_hint)),
                ),
            );
        } else if server {
            body = body.child(field("Server URL", self.url.clone())).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(tr(cx, msg::plugins::plugins_add_server_auth)),
                    )
                    .child(
                        div()
                            .id("plugin-auth-tabs")
                            .role(Role::TabList)
                            .aria_label(tr(cx, msg::plugins::plugins_add_server_auth))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .p(px(2.))
                            .gap(px(2.))
                            .rounded(px(7.))
                            .bg(crate::settings_style::frame_bg().opacity(0.55))
                            .children(
                                [
                                    (ServerAuth::Automatic, "Sign in or none"),
                                    (ServerAuth::Bearer, "Bearer token"),
                                    (ServerAuth::Header, "Custom header"),
                                ]
                                .into_iter()
                                .enumerate()
                                .map(|(i, (auth, title))| {
                                    let handles = self.auth_focus.clone();
                                    div()
                                        .id(("plugin-auth", i))
                                        .role(Role::Tab)
                                        .aria_label(title)
                                        .aria_selected(self.auth == auth)
                                        .track_focus(
                                            &self.auth_focus[i].clone().tab_stop(self.auth == auth),
                                        )
                                        .tab_index(if self.auth == auth { 0 } else { -1 })
                                        .cursor_pointer()
                                        .flex_1()
                                        .min_w_0()
                                        .h(px(24.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .px(px(10.))
                                        .rounded(px(5.))
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if self.auth == auth {
                                            self.colors.text
                                        } else {
                                            self.colors.muted
                                        })
                                        .when(self.auth == auth, |d| {
                                            d.bg(crate::palette::rgb(crate::shell::BG))
                                        })
                                        .child(title)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.busy {
                                                this.auth = auth;
                                                this.error = None;
                                                cx.notify();
                                            }
                                        }))
                                        .on_key_down(cx.listener(
                                            move |this, event: &KeyDownEvent, window, cx| {
                                                let index = match event.keystroke.key.as_str() {
                                                    "right" => (i + 1) % 3,
                                                    "left" => (i + 2) % 3,
                                                    "home" => 0,
                                                    "end" => 2,
                                                    _ => return,
                                                };
                                                if !this.busy {
                                                    this.auth = [
                                                        ServerAuth::Automatic,
                                                        ServerAuth::Bearer,
                                                        ServerAuth::Header,
                                                    ][index];
                                                    this.error = None;
                                                    window.focus(&handles[index], cx);
                                                    cx.notify();
                                                }
                                                cx.stop_propagation();
                                            },
                                        ))
                                }),
                            ),
                    )
                    .when(self.auth == ServerAuth::Automatic, |d| {
                        d.child(
                            div()
                                .text_color(self.colors.muted)
                                .child(tr(cx, msg::plugins::plugins_add_server_auth_auto_hint)),
                        )
                    }),
            );
        }
        if server && !local && self.auth == ServerAuth::Header {
            body = body.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(field("Header name", self.header.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex_grow(1.5)
                            .min_w_0()
                            .child(field("Value", self.secret.clone())),
                    ),
            );
        } else if needs_key {
            body = body.child(field(
                if server { "Token" } else { "API key" },
                self.secret.clone(),
            ));
        }
        if server && needs_key {
            body = body.child(
                div()
                    .text_color(self.colors.muted)
                    .child(tr(cx, msg::plugins::plugins_add_server_key_hint)),
            );
        }
        body = body.when_some(self.error, |d, error| {
            d.child(
                div()
                    .id("plugin-setup-error")
                    .role(Role::Alert)
                    .rounded(px(7.))
                    .bg(self.colors.danger.opacity(0.08))
                    .px(px(10.))
                    .py(px(6.))
                    .line_height(px(20.))
                    .text_color(self.colors.danger)
                    .child(error),
            )
        });
        let form = div()
            .id("plugin-setup-form")
            .role(Role::Dialog)
            .aria_label(if local {
                ("Add local MCP server").to_string()
            } else if server {
                crate::localization::text("plugins-plugins_add_server", cx)
            } else {
                ("Connect plugin").to_string()
            })
            .track_focus(&self.dialog_focus)
            .tab_group()
            .tab_stop(false)
            .w(px(448.).min(viewport.width - px(32.)))
            .max_h(viewport.height - px(32.))
            .overflow_y_scroll()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        this.cancel(window, cx);
                        cx.stop_propagation();
                    }
                    "tab" => {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                        if !this.dialog_focus.contains_focused(window, cx) {
                            if event.keystroke.modifiers.shift {
                                window.focus(&this.submit_focus, cx);
                            } else if this.offers_local() {
                                window.focus(&this.transport_focus[this.local as usize], cx);
                            } else {
                                window.focus(&this.name.read(cx).focus_handle(cx), cx);
                            }
                        }
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(self.colors.border)
            .bg(self.colors.background)
            .text_color(self.colors.text)
            .text_size(px(12.))
            .line_height(px(18.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(16.))
                    .pb(px(10.))
                    .child(
                        div()
                            .pr(px(20.))
                            .text_size(px(15.))
                            .line_height(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if server {
                                crate::localization::text("plugins-plugins_add_server", cx)
                            } else {
                                ("Connect plugin").to_string()
                            }),
                    )
                    .when(server, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(self.colors.muted)
                                .child(if self.tenant.starts_with("organization:") {
                                    ("Adding to your organization workspace.").to_string()
                                } else {
                                    crate::localization::text(
                                        "plugins-plugins_add_server_workspace_personal",
                                        cx,
                                    )
                                }),
                        )
                    }),
            )
            .child(body)
            .when_some(self.menu.as_ref(), |d, menu| {
                d.child(crate::input::edit_menu::render(menu, cx))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .px(px(16.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(self.colors.border.opacity(0.6))
                    .bg(crate::settings_style::frame_bg().opacity(0.72))
                    .rounded_b(px(11.))
                    .child(
                        button("cancel-plugin-setup", "Cancel")
                            .track_focus(&self.cancel_focus)
                            .hover(|d| d.bg(self.colors.border.opacity(0.4)))
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.cancel(window, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    )
                    .child(
                        button(
                            "submit-plugin-setup",
                            if self.busy {
                                "Working…"
                            } else if server {
                                "Add server"
                            } else {
                                "Connect"
                            },
                        )
                        .track_focus(&self.submit_focus)
                        .border_1()
                        .border_color(crate::palette::rgb(0x89ca21))
                        .bg(crate::palette::rgb(0x89ca21))
                        .text_color(rgb(0x18210b))
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                        .on_key_down(cx.listener(
                            |this, event: &KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.submit(cx);
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .id("close-plugin-setup")
                    .role(Role::Button)
                    .aria_label(tr(cx, msg::common::close))
                    .cursor_pointer()
                    .absolute()
                    .top(px(8.))
                    .right(px(8.))
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .hover(|d| d.bg(self.colors.border.opacity(0.4)))
                    .child(
                        crate::assets::icon("close")
                            .size(px(14.))
                            .text_color(self.colors.muted),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
            );
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(px(0.), px(0.)))
                .child(
                    div()
                        .id("plugin-dialog-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .p(px(16.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .occlude()
                        .bg(black().opacity(0.32))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                if !this.busy {
                                    this.cancel(window, cx);
                                }
                                cx.stop_propagation();
                            }),
                        )
                        .child(div().flex_1())
                        .child(form)
                        .child(div().flex_1().flex_grow(3.)),
                ),
        )
        .priority(3)
        .into_any_element()
    }
}

impl crate::WorkspaceView {
    pub(super) fn open_plugin_setup(
        &mut self,
        target: SetupTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_state.active_attempt() {
            self.plugin_state.notice =
                Some("Finish or cancel the current plugin sign-in first.".into());
            cx.notify();
            return;
        }
        if !self.connected
            || self.plugin_request.is_some()
            || self.cloud_account["user"]["id"].as_str().is_none()
            || self.plugin_state.snapshot["canManage"] == false
        {
            return;
        }
        let Some(tenant) = self.plugin_state.snapshot["tenantId"]
            .as_str()
            .map(str::to_owned)
        else {
            return;
        };
        let owner = self.cloud_account["user"]["id"].clone();
        let expected = tenant.clone();
        let colors = SetupColors {
            text: crate::palette::rgb(crate::shell::TEXT).into(),
            muted: crate::palette::rgb(crate::shell::MUTED).into(),
            background: crate::palette::rgb(crate::shell::RAISED).into(),
            border: crate::settings_style::card_border(),
            danger: crate::palette::rgb(0xf87171).into(),
        };
        let return_to = if self.browser_local {
            PluginReturnTo::Desktop { port: 0 }
        } else {
            PluginReturnTo::Web
        };
        let form = cx.new(|cx| PluginSetup::new(tenant, target, return_to, colors, window, cx));
        self.plugin_setup_events = Some(cx.subscribe(&form, move |this, _, event, cx| {
            match event {
                SetupEvent::Cancel => {
                    this.plugin_setup = None;
                    this.plugin_setup_events = None;
                }
                SetupEvent::Submit(request) => {
                    if !this.connected
                        || this.plugin_request.is_some()
                        || this.cloud_account["user"]["id"] != owner
                        || this.plugin_state.snapshot["tenantId"] != expected
                        || this.plugin_state.snapshot["canManage"] == false
                    {
                        return;
                    }
                    this.plugin_setup = None;
                    this.plugin_setup_events = None;
                    if let Ok(value) = serde_json::to_value(request) {
                        this.plugin_action(value);
                    }
                }
            }
            cx.notify();
        }));
        self.plugin_setup = Some(form);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;
    use std::prelude::v1::test;
    fn fixture(
        cx: &mut TestAppContext,
    ) -> (
        WindowHandle<crate::WorkspaceView>,
        mpsc::UnboundedReceiver<serde_json::Value>,
        mpsc::UnboundedSender<serde_json::Value>,
    ) {
        fixture_in(cx, "tenant")
    }
    fn fixture_in(
        cx: &mut TestAppContext,
        tenant: &str,
    ) -> (
        WindowHandle<crate::WorkspaceView>,
        mpsc::UnboundedReceiver<serde_json::Value>,
        mpsc::UnboundedSender<serde_json::Value>,
    ) {
        cx.update(crate::initialize);
        let (requests, rx) = mpsc::unbounded();
        let (events, frames) = mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            crate::WorkspaceView::new(
                crate::Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, window, cx| {
                view.connected = true;
                view.connection_epoch = 1;
                view.cloud_account = serde_json::json!({"user":{"id":"owner"}});
                view.plugin_state.snapshot =
                    serde_json::json!({"kind":"snapshot","tenantId":tenant,"canManage":true});
                view.open_plugin_setup(SetupTarget::Server, window, cx);
            })
            .unwrap();
        (window, rx, events)
    }
    fn fill(
        view: &mut crate::WorkspaceView,
        window: &mut Window,
        cx: &mut Context<crate::WorkspaceView>,
    ) -> Entity<PluginSetup> {
        let form = view.plugin_setup.clone().unwrap();
        form.update(cx, |form, cx| {
            form.name
                .update(cx, |input, cx| input.set_value("Tools", window, cx));
            form.url.update(cx, |input, cx| {
                input.set_value("https://tools.example/mcp", window, cx)
            });
            form.secret
                .update(cx, |input, cx| input.set_value("fixture-key", window, cx));
            form.auth = ServerAuth::Bearer;
        });
        form
    }
    #[gpui::test]
    fn setup_mouse_focus_routes_text_to_each_field(cx: &mut TestAppContext) {
        let (window, _requests, _events) = fixture(cx);
        window
            .update(cx, |view, _, cx| {
                view.settings = true;
                view.settings_section = crate::settings_view::Section::Plugins;
                cx.notify();
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear());
        let name = cx.debug_bounds("Name").unwrap();
        cx.simulate_click(name.center(), Modifiers::default());
        cx.simulate_input("Tools");
        cx.update(|window, cx| window.draw(cx).clear());
        let url = cx.debug_bounds("Server URL").unwrap();
        cx.simulate_click(url.center(), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_input("https://tools.example/mcp");
        window
            .update(cx, |view, _, cx| {
                let form = view.plugin_setup.as_ref().unwrap().read(cx);
                assert_eq!(form.name.read(cx).value().as_ref(), "Tools");
                assert_eq!(
                    form.url.read(cx).value().as_ref(),
                    "https://tools.example/mcp"
                );
            })
            .unwrap();
        window.update(cx,|view,window,cx|{
            let plugin=serde_json::from_value(serde_json::json!({"id":"key-tools","name":"Key tools","description":"Fixture","apiKeyHeader":"Authorization"})).unwrap();
            view.settings=false;
            view.plugins_browse=true;
            view.plugin_detail=Some("key-tools".into());
            view.plugin_state.snapshot["catalog"]=serde_json::to_value([&plugin]).unwrap();
            view.open_plugin_setup(SetupTarget::Catalog(plugin),window,cx);
        }).unwrap();
        cx.update(|window, cx| window.draw(cx).clear());
        let key = cx.debug_bounds("API key").unwrap();
        cx.simulate_click(key.center(), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_input("fixture-key");
        window
            .update(cx, |view, _, cx| {
                let form = view.plugin_setup.as_ref().unwrap().read(cx);
                assert_eq!(form.name.read(cx).value().as_ref(), "Key tools");
                assert_eq!(form.secret.read(cx).value().as_ref(), "fixture-key");
            })
            .unwrap();
    }
    #[gpui::test]
    fn setup_submits_once_and_discards_the_credential_form(cx: &mut TestAppContext) {
        let (window, mut requests, _events) = fixture(cx);
        window
            .update(cx, |view, window, cx| {
                let form = fill(view, window, cx);
                form.update(cx, |form, cx| form.submit(cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert!(view.plugin_setup.is_none());
                assert!(view.plugin_setup_events.is_none());
                assert!(view.plugin_request.is_some());
            })
            .unwrap();
        let mut writes = vec![];
        while let Ok(request) = requests.try_recv() {
            if request["method"] == zuse_protocol::methods::PLUGINS {
                writes.push(request);
            }
        }
        assert_eq!(writes.len(), 1);
        let params = &writes[0]["params"];
        assert_eq!(params["tenantId"], "tenant");
        assert_eq!(params["apiKey"]["value"], "Bearer fixture-key");
        assert_eq!(params["returnTo"]["kind"], "web");
    }
    #[gpui::test]
    fn setup_rejects_stale_account_tenant_and_management_authority(cx: &mut TestAppContext) {
        for condition in 0..4 {
            let (window, mut requests, _events) = fixture(cx);
            window
                .update(cx, |view, window, cx| {
                    let form = fill(view, window, cx);
                    match condition {
                        0 => view.cloud_account = serde_json::json!({"user":{"id":"another"}}),
                        1 => view.plugin_state.snapshot["tenantId"] = serde_json::json!("another"),
                        2 => view.plugin_state.snapshot["canManage"] = serde_json::json!(false),
                        _ => view.connected = false,
                    }
                    form.update(cx, |form, cx| form.submit(cx));
                })
                .unwrap();
            cx.run_until_parked();
            while let Ok(request) = requests.try_recv() {
                assert_ne!(request["method"], zuse_protocol::methods::PLUGINS);
            }
        }
    }
    fn plugin_writes(
        requests: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    ) -> Vec<serde_json::Value> {
        let mut writes = vec![];
        while let Ok(request) = requests.try_recv() {
            if request["method"] == zuse_protocol::methods::PLUGINS {
                writes.push(request["params"].clone());
            }
        }
        writes
    }
    #[gpui::test]
    fn local_transport_submits_a_stdio_server_for_personal_workspaces(cx: &mut TestAppContext) {
        let (window, mut requests, _events) = fixture_in(cx, "personal:owner");
        window
            .update(cx, |view, _, cx| {
                view.settings = true;
                view.settings_section = crate::settings_view::Section::Plugins;
                cx.notify();
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("MCP configuration (JSON)").is_none());
        let local = cx.debug_bounds("plugin-transport-Local").unwrap();
        cx.simulate_click(local.center(), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("Server URL").is_none());
        assert!(cx.debug_bounds("Name").is_some());
        let config = cx.debug_bounds("MCP configuration (JSON)").unwrap();
        cx.simulate_click(config.center(), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        window
            .update(cx, |view, window, cx| {
                let form = view.plugin_setup.clone().unwrap();
                form.update(cx, |form, cx| {
                    assert!(form.local);
                    assert_eq!(form.config.read(cx).value().as_ref(), LOCAL_CONFIG_TEMPLATE);
                    assert!(form.config.read(cx).focus_handle(cx).is_focused(window));
                    form.config
                        .update(cx, |input, cx| input.set_value("", window, cx));
                });
            })
            .unwrap();
        // Enter in the configuration inserts a line break instead of submitting.
        cx.simulate_input("{");
        cx.simulate_keystrokes("enter");
        cx.simulate_input(
            r#""mcpServers":{"files":{"type":"stdio","command":"npx","args":["-y","@scope/server"],"env":{"TOKEN":"fixture-value"}}}}"#,
        );
        window
            .update(cx, |view, window, cx| {
                let form = view.plugin_setup.clone().unwrap();
                form.update(cx, |form, cx| {
                    assert!(form.config.read(cx).value().starts_with("{\n"));
                    form.name
                        .update(cx, |input, cx| input.set_value("Files", window, cx));
                    form.submit(cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        let writes = plugin_writes(&mut requests);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["action"], "addLocalServer");
        assert_eq!(writes[0]["tenantId"], "personal:owner");
        assert_eq!(writes[0]["command"], "npx");
        assert_eq!(
            writes[0]["args"],
            serde_json::json!(["-y", "@scope/server"])
        );
        assert_eq!(writes[0]["env"]["TOKEN"], "fixture-value");
        assert!(writes[0].get("url").is_none() && writes[0].get("returnTo").is_none());
        window
            .update(cx, |view, _, _| assert!(view.plugin_setup.is_none()))
            .unwrap();
    }
    #[gpui::test]
    fn organization_workspaces_only_offer_remote_servers(cx: &mut TestAppContext) {
        let (window, mut requests, _events) = fixture_in(cx, "organization:team");
        window
            .update(cx, |view, _, cx| {
                view.settings = true;
                view.settings_section = crate::settings_view::Section::Plugins;
                cx.notify();
            })
            .unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("plugin-transport-Local").is_none());
        window
            .update(cx, |view, window, cx| {
                let form = fill(view, window, cx);
                form.update(cx, |form, cx| {
                    form.set_local(true, cx);
                    assert!(!form.local);
                    form.submit(cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        let writes = plugin_writes(&mut requests);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["action"], "addServer");
    }
    #[gpui::test]
    fn invalid_local_input_stays_in_the_form_without_a_request(cx: &mut TestAppContext) {
        let (window, mut requests, _events) = fixture_in(cx, "personal:owner");
        window
            .update(cx, |view, window, cx| {
                let form = view.plugin_setup.clone().unwrap();
                form.update(cx, |form, cx| {
                    form.set_local(true, cx);
                    form.name
                        .update(cx, |input, cx| input.set_value("Files", window, cx));
                    // The untouched template has no command yet.
                    form.submit(cx);
                    assert_eq!(
                        form.error,
                        Some(zuse_protocol::plugin_setup::LOCAL_CONFIG_INVALID)
                    );
                    form.config.update(cx, |input, cx| {
                        input.set_value(
                            r#"{"command":"npx","env":{"BAD NAME":"fixture-value"}}"#,
                            window,
                            cx,
                        )
                    });
                    form.submit(cx);
                    assert!(form.error.is_some_and(|e| !e.contains("fixture-value")));
                    form.config.update(cx, |input, cx| {
                        input.set_value(
                            r#"{"mcpServers":{"a":{"command":"x"},"b":{"command":"y"}}}"#,
                            window,
                            cx,
                        )
                    });
                    form.submit(cx);
                    assert_eq!(
                        form.error,
                        Some(zuse_protocol::plugin_setup::LOCAL_CONFIG_INVALID)
                    );
                });
            })
            .unwrap();
        cx.run_until_parked();
        assert!(plugin_writes(&mut requests).is_empty());
        window
            .update(cx, |view, _, _| assert!(view.plugin_setup.is_some()))
            .unwrap();
    }
}
