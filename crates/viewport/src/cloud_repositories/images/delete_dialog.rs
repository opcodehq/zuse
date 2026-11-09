use super::*;
use crate::localization::message as tr;
use zuse_i18n::messages as msg;
impl RepositoryPanel {
    pub(super) fn with_image_delete(
        &mut self,
        mut view: Div,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if let Some(id) = self.image.confirm_delete.clone() {
            if let Some(focus) = self.image.confirm_focus.clone() {
                let card=s::card().id("image-delete-dialog").w(px(420.)).p_5().role(Role::Dialog).aria_label(tr(cx, msg::native::delete_saved_image))
                    .child(div().text_size(px(18.)).child(tr(cx, msg::native::delete_saved_image)))
                    .child(s::description_text(tr(cx, msg::native::this_removes_the_saved_cloud_image_and_stops_its_storage_charges_existing_worksp_8658360a)).mt_3())
                    .child(row().mt_4().gap_3().justify_end()
                        .child(s::button("cancel-image-delete",tr(cx, msg::common::cancel)).track_focus(&focus[1]).tab_index(0).on_click(cx.listener(|this,_,window,cx|this.dismiss_image_delete(window,cx))))
                        .child(s::button("confirm-image-delete",tr(cx, msg::native::delete_image)).track_focus(&focus[2]).tab_index(0).text_color(crate::palette::rgb(0xe5a0a0)).on_click(cx.listener(move|this,_,window,cx|{this.dismiss_image_delete(window,cx);this.delete_image(&id,cx);}))))
                    ;
                let overlay = div()
                    .w(window.viewport_size().width)
                    .h(window.viewport_size().height)
                    .occlude()
                    .bg(rgba(0x00000099))
                    .flex()
                    .items_center()
                    .justify_center()
                    .track_focus(&focus[0])
                    .key_context("CloudImageDialog")
                    .on_action(cx.listener(|this, _: &CancelImageDelete, window, cx| {
                        this.dismiss_image_delete(window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ConfirmImageDelete, window, cx| {
                        let confirmed = this
                            .image
                            .confirm_focus
                            .as_ref()
                            .is_some_and(|f| f[2].is_focused(window));
                        let id = this.image.confirm_delete.clone();
                        this.dismiss_image_delete(window, cx);
                        if confirmed {
                            if let Some(id) = id {
                                this.delete_image(&id, cx);
                            }
                        }
                    }))
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => this.dismiss_image_delete(window, cx),
                            "tab" => {
                                if let Some(f) = &this.image.confirm_focus {
                                    window
                                        .focus(&f[if f[1].is_focused(window) { 2 } else { 1 }], cx);
                                }
                            }
                            "enter" | "space" => {
                                let confirmed = this
                                    .image
                                    .confirm_focus
                                    .as_ref()
                                    .is_some_and(|f| f[2].is_focused(window));
                                let id = this.image.confirm_delete.clone();
                                this.dismiss_image_delete(window, cx);
                                if confirmed {
                                    if let Some(id) = id {
                                        this.delete_image(&id, cx);
                                    }
                                }
                            }
                            _ => {}
                        }
                        cx.stop_propagation();
                    }))
                    .child(card);
                view = view.child(deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .position(point(px(0.), px(0.)))
                        .child(overlay),
                ));
            }
        }
        view
    }
}
