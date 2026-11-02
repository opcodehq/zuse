//! Saved-chat cards and filtering shared by the global archive screens.
use crate::localization::message as tr;
use crate::{
    assets::icon,
    palette::rgb,
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use zuse_i18n::messages as msg;
use zuse_protocol::Chat;

pub const PAGE_SIZE: usize = 40;
pub struct Entry {
    pub chat: Chat,
    pub project: String,
    pub computer: Option<String>,
}
impl Entry {
    pub fn matches(&self, query: &str) -> bool {
        let text = format!(
            "{} {} {} {} {} {}",
            self.chat.title.as_deref().unwrap_or("Untitled chat"),
            self.chat
                .last_message_preview
                .as_deref()
                .unwrap_or_default(),
            self.chat.branch.as_deref().unwrap_or_default(),
            self.chat.cwd.as_deref().unwrap_or_default(),
            self.project,
            self.computer.as_deref().unwrap_or_default()
        );
        text.to_lowercase().contains(query)
    }
    pub fn date(&self) -> chrono::DateTime<chrono::Utc> {
        self.chat.last_message_at.unwrap_or(self.chat.created_at)
    }
}
#[derive(Clone)]
pub enum Action {
    Open(String),
    Restore(String),
    Menu(String),
    Page(usize),
}
pub struct State {
    pub preview: bool,
    pub menu: bool,
    pub restoring: bool,
    pub blocked: bool,
}
pub fn page(total: usize, requested: usize) -> usize {
    requested.min(total.saturating_sub(1) / PAGE_SIZE)
}
pub fn card<C: 'static>(
    entry: Entry,
    index: usize,
    state: State,
    run: fn(&mut C, Action, &mut Window, &mut Context<C>),
    cx: &mut Context<C>,
) -> AnyElement {
    let thread = crate::project_archive::Thread::from_chat(entry.chat.clone());
    let title = thread.title();
    let id = entry.chat.id;
    let mut context = vec![entry.project];
    if let Some(computer) = entry.computer {
        context.push(computer);
    }
    if let Some(branch) = entry.chat.branch.as_ref().filter(|s| !s.is_empty()) {
        context.push(branch.clone());
    }
    if entry.chat.parent_chat_id.is_some() {
        context.push("Side chat".into());
    }
    let mut body = div()
        .id(("saved-archive-open", index))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(40.))
        .child(
            div()
                .size(px(32.))
                .flex_none()
                .rounded(px(8.))
                .bg(s::frame_bg())
                .flex()
                .items_center()
                .justify_center()
                .child(icon("project-archive").size(px(16.)).text_color(rgb(MUTED))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT))
                        .child(title.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(context.join(" · ")),
                )
                .when(
                    entry
                        .chat
                        .last_message_preview
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty()),
                    |v| {
                        v.child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child(thread.summary()),
                        )
                    },
                ),
        );
    if state.preview {
        let open = id.clone();
        body = body
            .role(Role::Button)
            .aria_label(format!("Open {title}"))
            .tab_index(0)
            .cursor_pointer()
            .on_key_down(|event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                run(this, Action::Open(open.clone()), window, cx)
            }));
    }
    let restore = id.clone();
    let menu = id;
    div()
        .id(("saved-archive-card", index))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(s::card_border())
        .bg(s::card_bg())
        .line_height(px(16.))
        .child(body)
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(thread.activity_label()),
                )
                .child(
                    s::button(
                        ("saved-archive-restore", index),
                        if state.restoring {
                            "Restoring…"
                        } else {
                            "Restore chat"
                        },
                    )
                    .tab_index(0)
                    .px(px(10.))
                    .when(state.blocked, |v| v.opacity(0.5).cursor_default())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !state.blocked {
                            run(this, Action::Restore(restore.clone()), window, cx);
                        }
                    })),
                ),
        )
        .when(state.menu, |v| {
            v.child(
                s::button(("saved-archive-menu", index), "")
                    .size(px(28.))
                    .px_0()
                    .tab_index(0)
                    .aria_label(format!("Actions for {title}"))
                    .child(icon("more-horizontal").size(px(14.)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        run(this, Action::Menu(menu.clone()), window, cx)
                    })),
            )
        })
        .into_any_element()
}
pub fn pagination<C: 'static>(
    total: usize,
    requested: usize,
    run: fn(&mut C, Action, &mut Window, &mut Context<C>),
    cx: &mut Context<C>,
) -> Option<AnyElement> {
    if total <= PAGE_SIZE {
        return None;
    }
    let current = page(total, requested);
    let next = page(total, current + 1);
    Some(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .py(px(8.))
            .text_size(px(11.))
            .text_color(rgb(MUTED))
            .child(
                s::button(
                    "saved-archive-previous",
                    tr(cx, msg::projects::masters_memory_previous),
                )
                .tab_index(0)
                .when(current == 0, |v| v.opacity(0.4))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if current > 0 {
                        run(this, Action::Page(current - 1), window, cx);
                    }
                })),
            )
            .child(format!(
                "{}–{} of {}",
                current * PAGE_SIZE + 1,
                ((current + 1) * PAGE_SIZE).min(total),
                total
            ))
            .child(
                s::button(
                    "saved-archive-next",
                    tr(cx, msg::chat::next_unread_button_next),
                )
                .tab_index(0)
                .when(current == next, |v| v.opacity(0.4))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if current < next {
                        run(this, Action::Page(next), window, cx);
                    }
                })),
            )
            .into_any_element(),
    )
}
