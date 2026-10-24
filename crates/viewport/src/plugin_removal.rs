//! Shared server removal prompt. The host validates and executes the selected removal.
use crate::plugin_setup::SetupColors;
use gpui::{prelude::*, *};

pub enum Event {
    Cancel,
    Remove,
}
pub struct ServerRemoval {
    name: String,
    pub busy: bool,
    pub error: Option<String>,
    colors: SetupColors,
    focus: FocusHandle,
    cancel: FocusHandle,
    remove: FocusHandle,
    return_focus: Option<FocusHandle>,
}
impl EventEmitter<Event> for ServerRemoval {}
impl ServerRemoval {
    pub fn new(
        name: String,
        colors: SetupColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let return_focus = window.focused(cx);
        let cancel = cx.focus_handle();
        window.focus(&cancel, cx);
        Self {
            name,
            colors,
            focus: cx.focus_handle(),
            cancel,
            remove: cx.focus_handle(),
            return_focus,
            busy: false,
            error: None,
        }
    }
    pub fn sync(&mut self, busy: bool, error: Option<String>, cx: &mut Context<Self>) {
        if self.busy != busy || self.error != error {
            self.busy = busy;
            self.error = error;
            cx.notify();
        }
    }
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(focus) = self.return_focus.clone() {
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        cx.emit(Event::Cancel);
    }
}
impl Render for ServerRemoval {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let c = self.colors;
        let button = |id: &'static str, title: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(title)
                .h(px(28.))
                .px(px(15.))
                .border_1()
                .border_color(transparent_black())
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(title)
                .when(self.busy, |d| d.opacity(0.5))
        };
        let form=div().id("plugin-server-removal").role(Role::AlertDialog).aria_label(format!("Remove {}?",self.name))
            .track_focus(&self.focus).tab_group().tab_stop(false).flex_none().w(px(384.).min(viewport.width-px(32.)))
            .max_h(viewport.height-px(32.)).overflow_y_scroll().rounded(px(12.)).border_1().border_color(c.border.opacity(0.6))
            .bg(c.background).text_color(c.text).flex().flex_col().occlude()
            .on_mouse_down(MouseButton::Left,|_,_,cx|cx.stop_propagation())
            .on_key_down(cx.listener(|this,event:&KeyDownEvent,window,cx| {
                match event.keystroke.key.as_str() {
                    "escape"=>this.cancel(window,cx),
                    "tab"=>{ let next=if this.cancel.is_focused(window) {&this.remove}else{&this.cancel};window.focus(next,cx); },
                    _=>return,
                };cx.stop_propagation();
            }))
            .child(div().p(px(16.)).flex().flex_col().gap(px(6.))
                .child(div().text_size(px(15.)).line_height(px(15.)).font_weight(FontWeight::SEMIBOLD).child(format!("Remove {}?",self.name)))
                .child(div().text_size(px(12.)).line_height(px(20.)).text_color(c.muted)
                    .child("This disconnects every account and removes the server from this workspace. Agents lose access right away."))
                .when_some(self.error.clone(),|d,error|d.child(div().id("plugin-form-error").role(Role::Alert).text_size(px(12.)).line_height(px(20.)).text_color(c.danger).child(error))))
            .child(div().border_t_1().border_color(c.border).bg(crate::settings_style::frame_bg()).rounded_b(px(11.))
                .px(px(16.)).py(px(10.)).flex().justify_end().gap(px(6.))
                .child(button("dismiss-plugin-removal","Cancel").track_focus(&self.cancel).tab_index(0)
                    .on_mouse_down(MouseButton::Left,{let focus=self.cancel.clone();move|_,window,cx|window.focus(&focus,cx)})
                    .hover(|d|d.bg(c.border.opacity(0.4)))
                    .on_click(cx.listener(|this,_,window,cx|this.cancel(window,cx)))
                    .on_key_down(cx.listener(|this,event:&KeyDownEvent,window,cx|if matches!(event.keystroke.key.as_str(),"enter"|"space") {this.cancel(window,cx);cx.stop_propagation();})))
                .child(button("confirm-plugin-removal",if self.busy {"Removing…"}else{"Remove server"}).track_focus(&self.remove).tab_index(0)
                    .on_mouse_down(MouseButton::Left,{let focus=self.remove.clone();move|_,window,cx|window.focus(&focus,cx)})
                    .bg(c.danger).text_color(white()).on_click(cx.listener(|this,_,_,cx|if !this.busy {cx.emit(Event::Remove)}))
                    .on_key_down(cx.listener(|this,event:&KeyDownEvent,_,cx|if matches!(event.keystroke.key.as_str(),"enter"|"space") {if !this.busy {cx.emit(Event::Remove)}cx.stop_propagation();}))));
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(px(0.), px(0.)))
                .child(
                    div()
                        .id("plugin-removal-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .p(px(16.))
                        .bg(black().opacity(0.32))
                        .flex()
                        .flex_col()
                        .items_center()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().flex_1())
                        .child(form)
                        .child(div().flex_1().flex_grow(3.)),
                ),
        )
        .priority(3)
    }
}
