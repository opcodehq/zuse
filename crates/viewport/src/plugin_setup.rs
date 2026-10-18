//! Shared ephemeral plugin setup form for both GPUI frontends.
use gpui::{prelude::*, *};
use gpui_base::input::{InputEditorStyle, InputEvent, InputState};
use zuse_protocol::{
    plugin_setup::{ServerAuth, ServerDraft, api_key},
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
    url: Entity<InputState>,
    header: Entity<InputState>,
    secret: Entity<InputState>,
    auth: ServerAuth,
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
        let name = field("Connection name", false);
        let url = field("https://example.com/mcp", false);
        let header = field("X-API-Key", false);
        let secret = field("API key", true);
        let subscriptions = [&name, &url, &header, &secret]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::ClipboardReadFailed) {
                        this.error = Some(crate::input::CLIPBOARD_PASTE_ERROR);
                    } else if matches!(event, InputEvent::Change)
                        && this.error == Some(crate::input::CLIPBOARD_PASTE_ERROR)
                    {
                        this.error = None;
                    }
                    // Focus changes must repaint the editor's platform text handler.
                    input.update(cx, |_, cx| cx.notify());
                    cx.notify();
                })
            })
            .collect();
        if let SetupTarget::Catalog(plugin) = &target {
            name.update(cx, |input, cx| {
                input.set_value(plugin.name.clone(), window, cx)
            });
        }
        window.focus(&name.read(cx).focus_handle(cx), cx);
        Self {
            tenant,
            target,
            return_to,
            name,
            url,
            header,
            secret,
            auth: ServerAuth::Automatic,
            error: None,
            menu: None,
            busy: false,
            colors,
            _inputs: subscriptions,
        }
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().to_string();
        let secret = self.secret.read(cx).value().to_string();
        let result = match &self.target {
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let server = matches!(self.target, SetupTarget::Server);
        let needs_key = match &self.target {
            SetupTarget::Catalog(p) => p.api_key_header.is_some(),
            SetupTarget::Server => self.auth != ServerAuth::Automatic,
        };
        let field = |title: &'static str, input: Entity<InputState>| {
            let focus = input.read(cx).focus_handle(cx);
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .text_color(self.colors.muted)
                        .text_size(px(11.))
                        .child(title),
                )
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
                        .rounded(px(6.))
                        .p(px(8.))
                        .child(input),
                )
        };
        let button = |id: &'static str, title: &'static str| {
            div()
                .id(id)
                .tab_index(0)
                .role(Role::Button)
                .cursor_pointer()
                .px(px(10.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(self.colors.border)
                .child(title)
        };
        div().id("plugin-setup-form").flex().flex_col().gap(px(12.)).p(px(16.)).rounded(px(10.)).border_1().border_color(self.colors.border).bg(self.colors.background).text_color(self.colors.text).text_size(px(12.))
            .child(div().font_weight(FontWeight::MEDIUM).child(if server {"Add MCP server"} else {"Connect plugin"}))
            .child(field("Name",self.name.clone()))
            .when(server,|d|d.child(field("Server URL",self.url.clone())).child(div().flex().flex_wrap().gap(px(6.)).children([(ServerAuth::Automatic,"Automatic"),(ServerAuth::Bearer,"Bearer token"),(ServerAuth::Header,"API key header")].into_iter().enumerate().map(|(i,(auth,title))| {
                div().id(("plugin-auth",i)).tab_index(0).role(Role::RadioButton).aria_label(title).cursor_pointer().px(px(10.)).py(px(5.)).rounded(px(6.)).border_1().border_color(if self.auth==auth {self.colors.text} else {self.colors.border}).child(title).on_click(cx.listener(move|this,_,_,cx|{if !this.busy {this.auth=auth;this.error=None;cx.notify();}}))
            }))))
            .when(server && self.auth==ServerAuth::Header,|d|d.child(field("Header name",self.header.clone())))
            .when(needs_key,|d|d.child(field("API key",self.secret.clone())))
            .when(server && self.auth==ServerAuth::Automatic,|d|d.child(div().text_size(px(11.)).text_color(self.colors.muted).child("Use the server’s sign-in flow, or connect without authentication when supported.")))
            .when_some(self.error,|d,error|d.child(div().id("plugin-setup-error").role(Role::Alert).text_color(self.colors.danger).child(error)))
            .when_some(self.menu.as_ref(), |d, menu| d.child(crate::input::edit_menu::render(menu,cx)))
            .child(div().flex().justify_end().gap(px(8.))
                .child(button("cancel-plugin-setup","Cancel").on_click(cx.listener(|_,_,_,cx|cx.emit(SetupEvent::Cancel))))
                .child(button("submit-plugin-setup",if self.busy {"Working…"} else {"Connect"}).on_click(cx.listener(|this,_,_,cx|this.submit(cx)))))
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
            background: crate::settings_style::card_bg(),
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
                    serde_json::json!({"kind":"snapshot","tenantId":"tenant","canManage":true});
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
}
