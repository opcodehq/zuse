//! GPUI browser controls and existing project-preview discovery. The platform
//! owns one isolated web surface; it never owns chat or engine mutations.
use super::*;
use crate::localization::message as tr;
use crate::shell::{column, control, icon_button, label, row};
use std::{cell::Cell, rc::Rc};
use zuse_i18n::messages as msg;

pub(super) struct Browser {
    address: Entity<input::TextInput>,
    _submit: Subscription,
    generation: u64,
    url: Option<String>,
    error: Option<String>,
    supported: Option<bool>,
    visible: bool,
    loading: bool,
    watch: Option<u64>,
    snapshot: zuse_protocol::PreviewSnapshot,
    bounds: Rc<Cell<Option<[f32; 4]>>>,
}
impl Browser {
    pub(super) fn new(window: &mut Window, cx: &mut Context<WorkspaceView>) -> Self {
        let address =
            cx.new(|cx| input::TextInput::new("Enter a project or website URL", window, cx));
        let submit = cx.subscribe(&address, |this, _, _: &input::Submitted, cx| {
            this.navigate_browser(false, cx)
        });
        Self {
            address,
            _submit: submit,
            generation: 0,
            url: None,
            error: None,
            supported: None,
            visible: false,
            loading: false,
            watch: None,
            snapshot: Default::default(),
            bounds: Rc::new(Cell::new(None)),
        }
    }
}
impl WorkspaceView {
    fn browser_command(&self, action: &str, extra: Value) {
        let mut frame = extra.as_object().cloned().unwrap_or_default();
        frame.insert("ui".into(), json!("browserFrame"));
        frame.insert("action".into(), json!(action));
        frame.insert("generation".into(), json!(self.browser.generation));
        let _ = self.requests.unbounded_send(Value::Object(frame));
    }
    pub(super) fn reset_browser(&mut self, cx: &mut Context<Self>) {
        self.browser.generation = self.browser.generation.wrapping_add(1);
        self.browser_command("close", json!({}));
        self.browser.url = None;
        self.browser.error = None;
        self.browser.loading = false;
        self.browser.visible = false;
        self.browser.bounds = Rc::new(Cell::new(None));
        if let Some(id) = self.browser.watch.take() {
            self.cancel(id);
        }
        self.browser.snapshot = Default::default();
        self.browser
            .address
            .update(cx, |input, cx| input.set_value(String::new(), cx));
    }
    pub(super) fn watch_browser_previews(&mut self) {
        if let Some(id) = self.browser.watch.take() {
            self.cancel(id);
        }
        self.browser.snapshot = Default::default();
        if !self.connected {
            return;
        }
        if let Some(chat) = &self.selected {
            // The viewing engine owns preview discovery and its peer proxy.
            // This intentionally uses the existing local catalog subscription.
            self.browser.watch =
                Some(self.call(methods::WATCH_PREVIEWS, json!({"chatId":chat["id"]})));
        }
    }
    pub(super) fn browser_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.right_open
            && self.right_tab == "browser"
            && self
                .browser
                .address
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
    }
    fn navigate_browser(&mut self, preview: bool, cx: &mut Context<Self>) {
        let url = self.browser.address.read(cx).value(cx).trim().to_owned();
        if url.is_empty() {
            return;
        }
        self.browser.generation = self.browser.generation.wrapping_add(1);
        self.browser.url = None;
        self.browser.error = None;
        self.browser.loading = true;
        self.browser.bounds = Rc::new(Cell::new(None));
        self.browser_command("navigate", json!({"url":url,"preview":preview}));
        // A new generation resets the platform visibility as well.
        self.browser.visible = false;
        cx.notify();
    }
    pub(super) fn receive_browser(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(event) = frame.get("browserFrame") else {
            return false;
        };
        if event["kind"] == "capability" {
            self.browser.supported = event["supported"].as_bool();
            cx.notify();
            return true;
        }
        if event["generation"].as_u64() != Some(self.browser.generation) {
            return true;
        }
        match event["kind"].as_str() {
            Some("navigated") => {
                if let Some(url) = event["url"].as_str() {
                    self.browser.url = Some(url.into());
                    self.browser
                        .address
                        .update(cx, |input, cx| input.set_value(url.into(), cx));
                }
            }
            Some("loaded") => self.browser.loading = false,
            Some("error") => {
                if let Some(url) = event["url"].as_str() {
                    self.browser.url = Some(url.into());
                }
                self.browser.error = Some(
                    event["message"]
                        .as_str()
                        .unwrap_or("The page could not open.")
                        .chars()
                        .take(1000)
                        .collect(),
                );
                self.browser.loading = false;
            }
            _ => {}
        }
        cx.notify();
        true
    }
    pub(super) fn receive_browser_previews(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.browser.watch != Some(id) {
            return false;
        }
        if frame.get("err").is_some() || frame["done"] == true {
            self.browser.watch = None;
            self.pending.remove(&id);
            self.browser.snapshot = Default::default();
        } else if let Some(item) = frame.get("item") {
            match serde_json::from_value::<zuse_protocol::PreviewSnapshot>(item.clone()) {
                Ok(snapshot) => self.browser.snapshot = snapshot,
                Err(_) => {
                    self.browser.snapshot = Default::default();
                    self.browser.watch = None;
                    self.cancel(id);
                }
            }
        }
        cx.notify();
        true
    }
    pub(super) fn sync_browser_visibility(&mut self) {
        let visible = self.right_open
            && self.right_tab == "browser"
            && !self.settings
            && !self.project_picker
            && self.preview_open.is_none()
            && self.thread_dialog.is_none()
            && !self.model_picker
            && !self.agent_picker
            && !self.access_picker
            && !self.add_menu
            && !self.context_menu;
        if !self.right_open || self.right_tab != "browser" || self.settings || self.project_picker {
            if let Some(id) = self.browser.watch.take() {
                self.cancel(id);
            }
        } else if visible && !self.browser.visible && self.browser.watch.is_none() {
            self.watch_browser_previews();
        }
        if self.browser.visible != visible {
            self.browser.visible = visible;
            self.browser_command("visible", json!({"visible":visible}));
        }
    }
    pub(super) fn browser_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_url = self.browser.url.is_some();
        let requests = self.requests.clone();
        let generation = self.browser.generation;
        let measured = self.browser.bounds.clone();
        column().flex_1().min_h_0()
            .child(row().h(px(42.)).px_2().gap_1().border_b_1().border_color(rgb(0x1f211f))
                .child(div().flex_1().min_w_0().child(self.browser.address.clone()))
                .child(icon_button("browser-go","arrow-right","Open address").on_click(cx.listener(|this,_,_,cx|this.navigate_browser(false,cx))))
                .child(icon_button("browser-reload","refresh","Reload page").opacity(if has_url{1.}else{0.4}).on_click(cx.listener(|this,_,_,cx|{if this.browser.url.is_some(){this.browser.loading=true;this.browser_command("reload",json!({}));cx.notify();}})))
                .when_some(self.browser.url.clone(),|d,url|d.child(icon_button("browser-external","arrow-up-right","Open page in a new tab").on_click(move|_,_,cx|cx.open_url(&url)))))
            .when_some(self.browser.error.clone(),|d,error|d.child(label(error).p_3().text_color(rgb(0xe5a0a0))))
            .when(self.browser.supported==Some(false),|d|d.child(label(tr(cx, msg::native::embedded_pages_are_unavailable_in_this_browser_open_the_url_in_a_new_tab)).p_3()))
            .when_some(self.browser.snapshot.error.clone(),|d,error|d.child(label(error).p_2()))
            .when(!self.browser.snapshot.services.is_empty(),|d|d.child(
                row().id("browser-project-previews").overflow_x_scroll().gap_2().p_2().children(self.browser.snapshot.services.iter().map(|service|{
                    let url=service.url(self.browser.snapshot.proxy_port);let can_open=self.browser_local&&self.browser.snapshot.error.is_none();let id=service.id.clone();
                    control(SharedString::from(format!("browser-preview-{id}")),format!("{} · {}",service.name,service.port)).opacity(if can_open{1.}else{0.5})
                        .on_click(cx.listener(move|this,_,_,cx|{if can_open{this.browser.address.update(cx,|input,cx|input.set_value(url.clone(),cx));this.navigate_browser(true,cx);}else{this.browser.error=Some("Enter the preview’s published URL to open it from this browser.".into());cx.notify();}}))
                }).collect::<Vec<_>>())
            ))
            .when(!has_url,|d|d.child(column().p_4().gap_2().child(label(tr(cx, msg::native::web_preview))).child(label(tr(cx, msg::native::open_a_running_project_preview_above_or_enter_a_website_address)))
                .when(!self.browser_local,|d|d.child(label(tr(cx, msg::native::project_proxy_urls_are_local_to_the_execution_computer_use_a_published_url_from_f4d971d6))))))
            .when(self.browser.loading,|d|d.child(label(tr(cx, msg::native::loading_page)).px_3().py_1()))
            .child(div().flex_1().min_h_0().overflow_hidden().child(canvas(move|_,_,_|{},move|bounds,_,_,_|{
                let rect=[f32::from(bounds.left()),f32::from(bounds.top()),f32::from(bounds.size.width),f32::from(bounds.size.height)];
                if measured.get()!=Some(rect){measured.set(Some(rect));let _=requests.unbounded_send(json!({"ui":"browserFrame","action":"bounds","generation":generation,"bounds":rect}));}
            }).size_full()))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn browser_navigation_keeps_composer_and_host_scope_separate(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut rx) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, _, cx| {
                view.connected = true;
                view.device_id = Some("viewer".into());
                view.selected = Some(json!({"id":"thread","deviceId":"host"}));
                view.browser.address.update(cx, |input, cx| {
                    input.set_value("http://example.localhost:3000".into(), cx)
                });
                view.input
                    .update(cx, |input, cx| input.set_value("Keep my draft".into(), cx));
            })
            .unwrap();
        window.update(cx,|view,window,cx|{
            view.set_panel("browser",cx);view.navigate_browser(false,cx);let generation=view.browser.generation;
            view.receive(json!({"browserFrame":{"generation":generation,"kind":"navigated","url":"http://example.localhost:3000/"}}),cx);
            assert!(view.browser.url.is_some());assert_eq!(view.input.read(cx).value(cx),"Keep my draft");
            window.focus(&view.browser.address.read(cx).focus_handle(cx),cx);assert!(view.browser_input_focused(window,cx));
            view.reset_browser(cx);view.receive(json!({"browserFrame":{"generation":generation,"kind":"navigated","url":"https://late.invalid"}}),cx);assert!(view.browser.url.is_none());
        }).unwrap();
        while let Ok(frame) = rx.try_recv() {
            assert_ne!(frame["method"], methods::QUEUE_MESSAGE);
            assert_ne!(frame["method"], methods::QUEUE_COMMAND);
            if frame["method"] == methods::WATCH_PREVIEWS {
                assert_eq!(frame["params"]["chatId"], "thread");
                assert!(frame["params"].get("targetDeviceId").is_none());
            }
        }
    }
}
