//! Queue rows share their delivery policy with the native composer.
use super::*;
use crate::tooltip::TooltipExt as _;
use crate::{
    assets::icon,
    shell::{column, control, icon_button, label, row},
};
use zuse_protocol::presentation::{QueueAction, queue_row_details};

/// A drain error belongs to the retained head row, not every queued message.
/// Never render a server string directly: unknown codes get a static fallback.
pub(super) fn retain_queue_error(items: &mut [Value], error: &Value) {
    for item in items.iter_mut() {
        if let Some(row) = item.as_object_mut() {
            row.remove("_cloudQueueError");
        }
    }
    if let Some(code) = error.as_str().filter(|s| !s.is_empty()) {
        if let Some(row) = items.first_mut().and_then(Value::as_object_mut) {
            row.insert("_cloudQueueError".into(), json!(code));
        }
    }
}

fn queue_error_label(code: Option<&str>) -> Option<&'static str> {
    Some(match code.filter(|s| !s.is_empty())? {
        "cloud_task_queue_goal_execution_unavailable" => "Goal unavailable · edit or remove",
        "cloud_task_queue_relative_reference_required" => "Fix file references · edit or remove",
        "cloud_task_input_too_large" => "Message too long · edit or remove",
        "cloud_task_queue_author_changed" | "workspace_sharing_changed" => {
            "Check account access · message kept"
        }
        "cloud_task_queue_runtime_recovery_required" => "Review interrupted action to continue",
        "cloud_task_session_config_invalid" => "Check agent and model · message kept",
        _ => "Message kept · edit or remove",
    })
}

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
            .when(self.cloud_server_commands(), |view| {
                view.child(
                    row()
                        .w_full()
                        .justify_between()
                        .px_2()
                        .py_1()
                        .child(
                            label(if self.queue_paused {
                                "Queue paused"
                            } else {
                                "Queued messages"
                            })
                            .text_size(px(11.)),
                        )
                        .child(
                            control(
                                "cloud-queue-pause",
                                if self.queue_paused {
                                    "Resume queue"
                                } else {
                                    "Pause queue"
                                },
                            )
                            .when(blocked, |view| view.opacity(0.35))
                            .when(!blocked, |view| {
                                view.on_click(cx.listener(|this, _, _, cx| {
                                    this.pause_cloud_queue(!this.queue_paused, cx)
                                }))
                            }),
                        ),
                )
            })
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
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_ellipsis()
                                    .child(details.title.clone()),
                            )
                            .when_some(
                                queue_error_label(item["_cloudQueueError"].as_str()),
                                |view, message| {
                                    view.child(
                                        div()
                                            .id(("queued-message-error", i))
                                            .role(Role::Status)
                                            .aria_label(message)
                                            .text_size(px(10.))
                                            .text_color(rgb(0xe2b876))
                                            .text_ellipsis()
                                            .child(message),
                                    )
                                },
                            ),
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
                        icon_button(
                            ("edit-queued", i),
                            "pen",
                            crate::localization::text("chat-queue_chip_edit_queued_message", cx),
                        )
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
                                .tip(action.tooltip())
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
                        icon_button(
                            ("remove-queued", i),
                            "queue-close",
                            crate::localization::text("chat-queue_chip_remove_queued_message", cx),
                        )
                        .size(px(24.))
                        .when(blocked || !details.allows(QueueAction::Remove), |view| {
                            view.opacity(0.35)
                        })
                        .when(
                            !blocked && details.allows(QueueAction::Remove),
                            |view| {
                                view.on_click(cx.listener(move |this, _, _, cx| {
                                    this.queue_action(QueueAction::Remove, &remove, None, cx)
                                }))
                            },
                        ),
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

#[cfg(test)]
mod error_tests {
    use super::*;
    use std::prelude::v1::test;

    #[test]
    fn blocked_head_retains_input_and_following_rows_stay_editable() {
        let original = json!({"id":"head","input":{"text":"keep draft","fileRefs":["src/main.rs"]},"revision":4});
        let second = json!({"id":"next","input":{"text":"next"},"revision":1});
        let mut rows = vec![original.clone(), second.clone()];
        retain_queue_error(
            &mut rows,
            &json!("cloud_task_queue_relative_reference_required"),
        );
        assert_eq!(rows[0]["input"], original["input"]);
        assert_eq!(rows[0]["revision"], 4);
        assert_eq!(rows[1], second);
        let details = queue_row_details(&rows[0]);
        assert!(details.valid_id && matches!(details.blocked, None | Some("Review required")));
        assert!(details.allows(QueueAction::Remove));
        assert_eq!(
            queue_error_label(rows[0]["_cloudQueueError"].as_str()),
            Some("Fix file references · edit or remove")
        );
        retain_queue_error(&mut rows, &Value::Null);
        assert_eq!(rows, vec![original, second]);
    }

    #[test]
    fn error_labels_never_expose_unrecognized_server_content() {
        assert_eq!(queue_error_label(None), None);
        assert_eq!(queue_error_label(Some("")), None);
        assert_eq!(
            queue_error_label(Some("unexpected secret or SQL details")),
            Some("Message kept · edit or remove")
        );
        assert_eq!(
            queue_error_label(Some("cloud_task_queue_runtime_recovery_required")),
            Some("Review interrupted action to continue")
        );
    }
}
