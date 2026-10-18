//! Queue rows share their delivery policy with the native composer.
use super::*;
use crate::{
    assets::icon,
    shell::{column, icon_button, label, row},
};
use zuse_protocol::presentation::{QueueAction, queue_row_details};

#[derive(Clone)]
pub(super) struct QueueDrag {
    pub chat: String,
    pub host: Option<String>,
    pub id: String,
    pub title: String,
}
impl Render for QueueDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(8.))
            .bg(rgb(crate::shell::RAISED))
            .text_size(px(12.))
            .child(self.title.clone())
    }
}
struct QueueTooltip(&'static str);
impl Render for QueueTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .bg(rgb(crate::shell::RAISED))
            .text_color(rgb(crate::shell::TEXT))
            .text_size(px(11.))
            .child(self.0)
    }
}
impl WorkspaceView {
    pub(super) fn queue_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let blocked = self.queue_action_blocked();
        column()
            .id("composer-queue-items")
            .w_full()
            .min_w_0()
            .max_h(px(188.))
            .overflow_y_scroll()
            .p(px(4.))
            .pb(px(12.))
            .mb(px(-10.))
            .gap(px(4.))
            .rounded_t(px(16.))
            .border_1()
            .border_b_0()
            .border_color(rgb(0x30312b))
            .bg(rgb(0x1c1d19))
            .children(self.queue.iter().enumerate().map(|(i, item)| {
                let details = queue_row_details(item);
                let edit_enabled = !blocked
                    && details.valid_id
                    && matches!(details.blocked, None | Some("Review required"));
                let primary = self.queued_primary_action(item);
                let drag = QueueDrag {
                    chat: self
                        .selected
                        .as_ref()
                        .and_then(|c| c["id"].as_str())
                        .unwrap_or_default()
                        .into(),
                    host: self
                        .selected
                        .as_ref()
                        .and_then(|c| c["deviceId"].as_str())
                        .map(str::to_owned),
                    id: item["id"].as_str().unwrap_or_default().into(),
                    title: details.title.clone(),
                };
                let destination = drag.id.clone();
                let move_enabled = !blocked && details.allows(QueueAction::Move);
                let edit = item.clone();
                let send = item.clone();
                let remove = item.clone();
                row()
                    .id(("queued-message", i))
                    .h(px(32.))
                    .px(px(4.))
                    .gap(px(4.))
                    .rounded(px(11.))
                    .hover(|view| view.bg(rgb(crate::shell::RAISED)))
                    .child(
                        div()
                            .id(("queue-drag", i))
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                icon("queue-drag-handle")
                                    .size(px(14.))
                                    .text_color(rgb(crate::shell::MUTED)),
                            )
                            .when(move_enabled, |view| {
                                view.cursor(gpui::CursorStyle::OpenHand).on_drag(
                                    drag,
                                    |drag, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.new(|_| drag.clone())
                                    },
                                )
                            })
                            .when(!move_enabled, |view| view.opacity(0.35)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_ellipsis()
                            .child(details.title.clone()),
                    )
                    .when_some(details.blocked, |view, status| {
                        view.child(label(status).text_size(px(10.)).text_color(rgb(0xe2b876)))
                    })
                    .when(details.attachment_count > 0, |view| {
                        view.child(
                            row()
                                .gap_1()
                                .child(icon("queue-paperclip").size(px(13.)))
                                .child(
                                    label(details.attachment_count.to_string()).text_size(px(10.)),
                                ),
                        )
                    })
                    .child(
                        icon_button(("edit-queued", i), "pen", "Edit queued message")
                            .size(px(24.))
                            .when(!edit_enabled, |view| view.opacity(0.35))
                            .when(edit_enabled, |view| {
                                view.on_click(cx.listener(move |this, _, _, cx| {
                                    this.begin_queue_edit(edit.clone(), cx)
                                }))
                            }),
                    )
                    .when_some(primary, |view, action| {
                        view.child(
                            icon_button(("queued-primary", i), "queue-send", action.tooltip())
                                .size(px(24.))
                                .aria_label(action.label())
                                .aria_description(action.tooltip())
                                .tooltip(move |_, cx| {
                                    cx.new(|_| QueueTooltip(action.tooltip())).into()
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.queue_action(action.action(), &send, None, cx)
                                })),
                        )
                    })
                    .when(primary.is_none(), |view| {
                        view.child(
                            icon_button(
                                ("queued-primary", i),
                                "queue-send",
                                "Waiting for queue confirmation",
                            )
                            .size(px(24.))
                            .opacity(0.35),
                        )
                    })
                    .child(
                        icon_button(("remove-queued", i), "queue-close", "Remove queued message")
                            .size(px(24.))
                            .when(blocked || !details.allows(QueueAction::Remove), |view| {
                                view.opacity(0.35)
                            })
                            .when(!blocked && details.allows(QueueAction::Remove), |view| {
                                view.on_click(cx.listener(move |this, _, _, cx| {
                                    this.queue_action(QueueAction::Remove, &remove, None, cx)
                                }))
                            }),
                    )
                    .when(move_enabled, |view| {
                        view.on_drop(cx.listener(move |this, payload: &QueueDrag, _, cx| {
                            this.move_queued_drop(payload, &destination, cx)
                        }))
                    })
            }))
            .into_any_element()
    }
}
