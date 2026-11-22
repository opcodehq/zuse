//! The Background work tray row: one quiet line while a chat's subagents or
//! background processes run ("2 agents · 1 task", the longest-running
//! elapsed time), expanding to a row per item. Agent rows open the
//! subagent's transcript; stoppable processes carry a stop control.

use crate::background_work::{self, WorkItem, WorkKind};
use crate::localization::message as tr;
use crate::tooltip::TooltipExt as _;
use chrono::Utc;
use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*, px};
use zuse_i18n::messages as msg;
use zuse_rpc::methods;

use super::tray::TrayPill;
use super::{Composer, ComposerEvent};
use crate::icons::{self, icon};
use crate::theme::Theme;

const ROW_HEIGHT: f32 = 30.0;
const ROW_PAD_X: f32 = 12.0;

impl Composer {
    /// The selected chat's running subagents and processes, as one tray row.
    pub(crate) fn background_work_pill(&mut self, cx: &mut Context<Self>) -> Option<TrayPill> {
        let now = Utc::now();
        let (chat_id, work) = {
            let state = self.state.read(cx);
            let chat_id = state.selected_chat.clone()?;
            let tasks = zuse_protocol::view::background_tasks(state.session_for(&chat_id), now);
            (
                chat_id,
                background_work::running_work(&state.transcript, tasks),
            )
        };
        // A requested stop lasts until the task leaves the live set.
        self.tray
            .background_stops
            .retain(|id| work.iter().any(|item| item.key() == id));
        if work.is_empty() {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let open = self.tray.background_open.contains(&chat_id);
        let agents = work.iter().filter(|item| item.is_agent()).count();
        let tasks = work.len() - agents;
        let mut counts = Vec::new();
        if agents > 0 {
            counts.push(tr(cx, |l| {
                msg::native::background_work_agents_plural(l, agents)
            }));
        }
        if tasks > 0 {
            counts.push(tr(cx, |l| {
                msg::native::background_work_tasks_plural(l, tasks)
            }));
        }
        let elapsed = |item_start: Option<chrono::DateTime<Utc>>| {
            item_start.map(|start| {
                SharedString::from(crate::transcript::format_elapsed(
                    now.signed_duration_since(start).num_seconds(),
                ))
            })
        };
        let weak = cx.weak_entity();
        let toggle_chat = chat_id.clone();
        let label = tr(
            cx,
            if open {
                msg::native::background_work_collapse
            } else {
                msg::native::background_work_expand
            },
        );
        let pill = TrayPill::new(
            "tray-background-work",
            tr(cx, msg::chat::background_tasks_title),
        )
        .icon_element(crate::loaders::retro_loader(
            "tray-background-work-loader",
            12.0,
            theme.loader_glyph(),
        ))
        .subtitle(counts.join(" · "))
        .on_click(label, move |_, _, cx| {
            weak.update(cx, |this, cx| {
                if !this.tray.background_open.remove(&toggle_chat) {
                    this.tray.background_open.insert(toggle_chat.clone());
                }
                cx.notify();
            })
            .ok();
        })
        .action(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .pr(px(4.0))
                .font_family(theme.font_mono.clone())
                .text_size(px(11.0))
                .text_color(theme.text_muted)
                .children(elapsed(background_work::oldest_start(&work)))
                .child(
                    icon(if open {
                        icons::ALT_ARROW_UP
                    } else {
                        icons::ALT_ARROW_DOWN
                    })
                    .size(px(14.0))
                    .text_color(theme.text_muted),
                ),
        );
        if !open {
            return Some(pill);
        }
        let rows: Vec<AnyElement> = work
            .iter()
            .map(|item| {
                self.background_work_row(&chat_id, item, elapsed(item.started_at), &theme, cx)
            })
            .collect();
        Some(
            pill.expanded(Some(
                div()
                    .py(px(4.0))
                    .flex()
                    .flex_col()
                    .children(rows)
                    .into_any_element(),
            )),
        )
    }

    fn background_work_row(
        &self,
        chat_id: &str,
        item: &WorkItem,
        elapsed: Option<SharedString>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = theme.element_hover;
        let glyph = if item.is_agent() {
            icons::AGENT
        } else {
            icons::TERMINAL
        };
        let trailing: AnyElement = match &item.kind {
            WorkKind::Task { task_id, .. } if self.tray.background_stops.contains(task_id) => {
                crate::loaders::retro_loader(
                    SharedString::from(format!("tray-background-stopping-{task_id}")),
                    12.0,
                    theme.loader_glyph(),
                )
            }
            WorkKind::Task {
                task_id,
                stoppable: true,
            } => {
                let (chat_id, task_id) = (chat_id.to_owned(), task_id.clone());
                let group = SharedString::from(format!("tray-background-stop-{task_id}"));
                let label = item.label.to_string();
                div()
                    .id(group.clone())
                    .group(group.clone())
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .role(gpui::Role::Button)
                    .aria_label(tr(cx, |l| {
                        msg::chat::background_stop_task(l, label.as_str())
                    }))
                    .tip(tr(cx, |l| {
                        msg::chat::background_stop_task(l, label.as_str())
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.stop_background_work(&chat_id, &task_id, cx);
                    }))
                    // A filled square reads as stop; an outline reads as a checkbox.
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded(px(1.5))
                            .bg(theme.text_muted)
                            .group_hover(group, |s| s.bg(theme.text)),
                    )
                    .into_any_element()
            }
            WorkKind::Task { .. } => div().size(px(20.0)).flex_none().into_any_element(),
            WorkKind::Agent { .. } => div()
                .size(px(20.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    icon(icons::ALT_ARROW_RIGHT)
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                )
                .into_any_element(),
        };
        let row = div()
            .id(SharedString::from(format!(
                "tray-background-{}",
                item.key()
            )))
            .min_h(px(ROW_HEIGHT))
            .mx(px(4.0))
            .px(px(ROW_PAD_X - 4.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .rounded(px(7.0))
            .text_size(px(12.0))
            .child(
                icon(glyph)
                    .flex_none()
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .max_w(gpui::relative(0.6))
                            .truncate()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(theme.text)
                            .child(item.label.clone()),
                    )
                    .children(item.command.clone().map(|command| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(command)
                    })),
            )
            .children(elapsed.map(|elapsed| {
                div()
                    .flex_none()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(elapsed)
            }))
            .child(trailing);
        match &item.kind {
            WorkKind::Agent { doc_id } => {
                let (chat_id, doc_id, title) =
                    (chat_id.to_owned(), doc_id.clone(), item.label.to_string());
                row.cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .role(gpui::Role::Button)
                    .aria_label(tr(cx, msg::native::background_work_open_agent))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ComposerEvent::OpenSubagent {
                            chat_id: chat_id.clone(),
                            doc_id: doc_id.clone(),
                            title: title.clone(),
                        });
                    }))
                    .into_any_element()
            }
            WorkKind::Task { .. } => row
                .when_some(item.command.clone(), |row, command| row.tip(command))
                .into_any_element(),
        }
    }

    /// Ask the chat's host to stop one background process: a session
    /// command, so it reaches the device the chat runs on.
    fn stop_background_work(&mut self, chat_id: &str, task_id: &str, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if !self.tray.background_stops.insert(task_id.to_owned()) {
            return;
        }
        cx.notify();
        let params = serde_json::json!({
            "chatId": chat_id,
            "command": { "kind": "stopBackgroundTask", "taskId": task_id },
        });
        let task_id = task_id.to_owned();
        cx.spawn(async move |this, cx| {
            if let Err(error) = engine.client().call(methods::QUEUE_COMMAND, params).await {
                tracing::warn!(%error, "background task stop failed");
                this.update(cx, |this, cx| {
                    this.tray.background_stops.remove(&task_id);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}
