//! Shared project archive composition. Hosts own transcript reads and restoration.
use crate::{
    assets::icon,
    palette::rgb,
    shell::{BG, BORDER, MUTED, TEXT},
};
use gpui::{prelude::*, *};
use zuse_protocol::Chat;

#[derive(Clone)]
pub struct Thread {
    pub chat: Chat,
    pub date: chrono::DateTime<chrono::Utc>,
}
impl Thread {
    pub fn from_chat(chat: Chat) -> Self {
        let date = chat.last_message_at.unwrap_or(chat.created_at);
        Self { chat, date }
    }
    pub fn title(&self) -> String {
        self.chat
            .title
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Untitled chat".into())
    }
    pub fn summary(&self) -> String {
        self.chat
            .last_message_preview
            .as_deref()
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|text| !text.is_empty())
            .or_else(|| {
                self.chat
                    .branch
                    .as_ref()
                    .map(|branch| format!("Branch {branch}"))
            })
            .unwrap_or_else(|| "Saved conversation".into())
    }
    pub fn activity_label(&self) -> String {
        format!(
            "{} {}",
            if self.chat.last_message_at.is_some() {
                "Last active"
            } else {
                "Created"
            },
            date_label(self.date)
        )
    }
}
#[derive(Clone)]
pub enum Action {
    Open(String),
    Restore(String),
    Session(String),
    Back,
    Retry,
}

pub fn date_label(date: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Datelike;
    let date = date.with_timezone(&chrono::Local);
    date.format(if date.year() == chrono::Local::now().year() {
        "%b %-d"
    } else {
        "%b %-d, %Y"
    })
    .to_string()
}
pub fn rows(chats: &[Chat], project: &str, query: &str) -> Vec<Thread> {
    let needle = query.trim().to_lowercase();
    let mut rows: Vec<_> = chats
        .iter()
        .filter(|c| c.archived && c.thread_id.is_none() && c.space_id.as_deref() == Some(project))
        .filter(|c| c.is_top_level())
        .filter(|c| {
            c.title
                .as_deref()
                .unwrap_or("Untitled chat")
                .to_lowercase()
                .contains(&needle)
                || c.last_message_preview
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
                || c.branch
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
        })
        .cloned()
        .map(Thread::from_chat)
        .collect();
    rows.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.chat.id.cmp(&b.chat.id)));
    rows
}
fn activate<T: 'static>(
    action: Action,
    run: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
    cx: &Context<T>,
) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this, event: &KeyDownEvent, window, cx| {
        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
            run(this, action.clone(), window, cx);
            cx.stop_propagation();
        }
    })
}
fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .hover(|d| d.bg(crate::settings_style::frame_bg()))
        .focus_visible(|d| d.bg(crate::settings_style::frame_bg()))
}
fn restore_icon(busy: bool, margin: f32) -> AnyElement {
    let glyph = icon(if busy { "loading" } else { "project-unarchive" })
        .size(px(14.))
        .opacity(0.8);
    let glyph = if busy {
        glyph
            .with_animation(
                "project-archive-restoring",
                Animation::new(std::time::Duration::from_secs(1)).repeat(),
                |svg, progress| {
                    svg.with_transformation(Transformation::rotate(percentage(progress)))
                },
            )
            .into_any_element()
    } else {
        glyph.into_any_element()
    };
    div()
        .flex_none()
        .relative()
        .w(px(14. + 2. * margin))
        .h(px(14.))
        .child(
            div()
                .absolute()
                .left(px(margin))
                .top_0()
                .size(px(14.))
                .child(glyph),
        )
        .into_any_element()
}
pub fn centered(text: impl Into<SharedString>) -> Div {
    div()
        .flex_1()
        .min_h(px(256.))
        .flex()
        .items_center()
        .justify_center()
        .px(px(24.))
        .text_size(px(14.))
        .text_color(rgb(MUTED))
        .text_center()
        .child(text.into())
}

pub fn failure<T: 'static>(
    text: impl Into<SharedString>,
    cx: &mut Context<T>,
    run: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> AnyElement {
    div()
        .size_full()
        .min_h(px(256.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .px(px(24.))
        .text_size(px(14.))
        .text_color(rgb(MUTED))
        .child(text.into())
        .child(
            button("project-archive-retry", "Retry")
                .h(px(24.))
                .border_1()
                .border_color(rgb(BORDER))
                .child("Retry")
                .on_key_down(activate(Action::Retry, run, cx))
                .on_click(
                    cx.listener(move |this, _, window, cx| run(this, Action::Retry, window, cx)),
                ),
        )
        .into_any_element()
}
pub fn list<T: 'static>(
    project_name: &str,
    query: &str,
    search: AnyElement,
    rows: &[Thread],
    busy: Option<&str>,
    error: Option<(&str, &str)>,
    compact: bool,
    cx: &mut Context<T>,
    run: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> AnyElement {
    let available = busy.is_none();
    let mut entries = div()
        .id("project-archive-rows")
        .flex()
        .flex_col()
        .gap(px(8.));
    let mut previous = None;
    for (i, row) in rows.iter().enumerate() {
        let date = date_label(row.date);
        if previous.as_ref() != Some(&date) {
            entries = entries.child(
                div()
                    .mt(px(if i == 0 { 0. } else { 24. }))
                    .mb(px(4.))
                    .px(px(16.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .child(date.clone()),
            );
            previous = Some(date.clone());
        }
        let id = row.chat.id.clone();
        let restore = id.clone();
        let title = row.title();
        let working = busy == Some(id.as_str());
        entries = entries.child(
            div()
                .id(("project-archive-row", i))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(8.))
                .py(px(8.))
                .rounded(px(10.))
                .border_1()
                .border_color(crate::settings_style::card_border())
                .bg(crate::settings_style::card_bg())
                .hover(|d| d.bg(crate::settings_style::frame_bg().opacity(0.45)))
                .child(
                    div()
                        .id(("project-archive-open", i))
                        .role(Role::Button)
                        .aria_label(format!("Open {title}"))
                        .tab_index(0)
                        .flex_1()
                        .min_w_0()
                        .min_h(px(44.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .cursor_pointer()
                        .focus_visible(|d| d.bg(crate::settings_style::frame_bg().opacity(0.45)))
                        .child(
                            div()
                                .size(px(32.))
                                .flex_none()
                                .rounded(px(8.))
                                .bg(crate::settings_style::frame_bg())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(rgb(MUTED))
                                .child(icon("project-archive").size(px(16.))),
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
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(title.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(rgb(MUTED))
                                        .child(row.summary()),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(rgb(MUTED))
                                .child(row.activity_label()),
                        )
                        .on_key_down(activate(Action::Open(id.clone()), run, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            run(this, Action::Open(id.clone()), window, cx)
                        })),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(4.))
                        .child(
                            button(
                                ("project-archive-restore", i),
                                if working {
                                    "Restoring…"
                                } else {
                                    "Restore chat"
                                },
                            )
                            .aria_label(format!("Restore chat {title}"))
                            .when(busy.is_some(), |d| d.opacity(0.64))
                            .child(restore_icon(working, 0.))
                            .child(if working {
                                "Restoring…"
                            } else {
                                "Restore chat"
                            })
                            .on_key_down(cx.listener({
                                let id = restore.clone();
                                move |this, event: &KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        if available {
                                            run(this, Action::Restore(id.clone()), window, cx)
                                        }
                                        cx.stop_propagation();
                                    }
                                }
                            }))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if available {
                                        run(this, Action::Restore(restore.clone()), window, cx)
                                    }
                                },
                            )),
                        )
                        .when_some(
                            error
                                .filter(|(id, _)| *id == row.chat.id)
                                .map(|(_, text)| text),
                            |d, text| {
                                d.child(
                                    div()
                                        .id(("project-archive-restore-error", i))
                                        .role(Role::Alert)
                                        .max_w(px(256.))
                                        .text_size(px(12.))
                                        .text_color(rgb(0xf87171))
                                        .child(text.to_owned()),
                                )
                            },
                        ),
                ),
        );
    }
    let has_rows = !rows.is_empty();
    div()
        .id("project-archive-page")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .bg(rgb(BG))
        .text_color(rgb(TEXT))
        .line_height(px(20.))
        .child(
            div()
                .flex_none()
                .border_b_1()
                .border_color(rgb(BORDER))
                .px(px(if compact { 16. } else { 32. }))
                .py(px(20.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(icon("project-archive").size(px(20.)).text_color(rgb(MUTED)))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(20.))
                                        .line_height(px(28.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("Archived chats"),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.))
                                        .line_height(px(16.))
                                        .text_color(rgb(MUTED))
                                        .child(project_name.to_owned()),
                                ),
                        )
                        .child(div().flex_1())
                        .child(div().flex_none().rounded_full().px(px(9.)).py(px(2.)).bg(crate::settings_style::frame_bg())
                            .text_size(px(11.)).text_color(rgb(MUTED)).child(format!("{} {}",rows.len(),if query.trim().is_empty(){if rows.len()==1{"chat"}else{"chats"}}else{if rows.len()==1{"match"}else{"matches"}}))),
                )
                .child(
                    div()
                        .mt(px(20.))
                        .h(px(28.))
                        .max_w(px(576.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(crate::settings_style::card_bg())
                        .px(px(10.))
                        .text_size(px(12.))
                        .child(
                            icon("magnifer")
                                .size(px(16.))
                                .flex_none()
                                .text_color(rgb(MUTED)),
                        )
                        .child(search),
                ),
        )
        .child(
            div()
                .id("project-archive-list-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(if compact { 16. } else { 32. }))
                .py(px(20.))
                .when(has_rows, |d| d.child(entries))
                .when(!has_rows, |d| {
                    d.child(centered("").flex_col().gap(px(10.))
                        .child(icon("project-archive").size(px(28.)).text_color(rgb(MUTED)))
                        .child(div().text_color(rgb(TEXT)).font_weight(FontWeight::MEDIUM).child(if query.trim().is_empty(){"No archived chats yet"}else{"No matching chats"}))
                        .child(div().max_w(px(400.)).text_size(px(12.)).child(if query.trim().is_empty(){format!("Archived chats from {project_name} will appear here. Preview them before restoring.")}else{"Try another title, preview, or branch.".into()})))
                }),
        )
        .into_any_element()
}
pub fn preview<T: 'static>(
    thread: &Thread,
    sessions: &[Chat],
    selected: &str,
    body: AnyElement,
    loading: bool,
    error: Option<&str>,
    busy: bool,
    cx: &mut Context<T>,
    run: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> AnyElement {
    let mut tabs = div()
        .id("project-archive-sessions")
        .role(Role::TabList)
        .aria_label("Archived chat sessions")
        .h(px(44.))
        .flex_none()
        .flex()
        .items_stretch()
        .gap(px(4.))
        .px(px(12.))
        .overflow_x_scroll()
        .border_b_1()
        .border_color(rgb(BORDER).opacity(0.5));
    for (i, session) in sessions.iter().enumerate() {
        let id = session.id.clone();
        let current = id == selected;
        let name = session
            .title
            .clone()
            .unwrap_or_else(|| format!("Conversation {}", i + 1));
        tabs = tabs.child(
            div()
                .id(("project-archive-session", i))
                .role(Role::Tab)
                .aria_label(name.clone())
                .aria_selected(current)
                .tab_index(0)
                .max_w(px(224.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_2()
                .border_color(if current {
                    rgb(TEXT).into()
                } else {
                    transparent_black()
                })
                .text_size(px(12.))
                .text_color(if current { rgb(TEXT) } else { rgb(MUTED) })
                .cursor_pointer()
                .child(
                    icon(crate::shell::provider_icon(
                        &session
                            .config
                            .as_ref()
                            .map(|c| String::from(c.harness))
                            .unwrap_or_default(),
                    ))
                    .size(px(14.)),
                )
                .child(div().truncate().child(name))
                .on_key_down(activate(Action::Session(id.clone()), run, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    run(this, Action::Session(id.clone()), window, cx)
                })),
        );
    }
    let restore = thread.chat.id.clone();
    let back = Action::Back;
    div()
        .id("project-archive-preview")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .bg(rgb(BG))
        .text_color(rgb(TEXT))
        .child(
            div()
                .min_h(px(56.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(20.))
                .border_b_1()
                .border_color(rgb(BORDER).opacity(0.5))
                .child(
                    button("project-archive-back", "Back to archived chats")
                        .size(px(44.))
                        .px_0()
                        .child(icon("alt-arrow-left").size(px(16.)))
                        .on_key_down(activate(back, run, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            run(this, Action::Back, window, cx)
                        })),
                )
                .child(icon("project-archive").size(px(16.)).text_color(rgb(MUTED)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(thread.title()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(rgb(MUTED))
                                .child(thread.activity_label()),
                        ),
                ),
        )
        .child(tabs)
        .child(
            div().flex_1().min_h_0().px(px(12.)).child(
                div()
                    .h_full()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .child(if loading {
                        centered("Loading transcript…").into_any_element()
                    } else {
                        body
                    }),
            ),
        )
        .child(
            div()
                .flex_none()
                .border_t_1()
                .border_color(rgb(BORDER).opacity(0.6))
                .bg(rgb(BG).opacity(0.92))
                .px(px(16.))
                .py(px(8.))
                .child(
                    div()
                        .w_full()
                        .max_w(px(896.))
                        .mx_auto()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(icon("project-archive").size(px(16.)).text_color(rgb(MUTED)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(rgb(MUTED))
                                .child("Read-only preview. Restore this chat to continue working.")
                                .when_some(error, |d, error| {
                                    d.child(
                                        div()
                                            .id("project-archive-preview-error")
                                            .role(Role::Alert)
                                            .text_size(px(11.))
                                            .text_color(rgb(0xf87171))
                                            .child(error.to_owned()),
                                    )
                                }),
                        )
                        .child(
                            button(
                                "project-archive-preview-restore",
                                if busy {
                                    "Restoring…"
                                } else if error.is_some() {
                                    "Retry"
                                } else {
                                    "Restore chat"
                                },
                            )
                            .h(px(28.))
                            .px(px(16.))
                            .gap(px(4.))
                            .text_size(px(12.))
                            .bg(crate::settings_style::card_bg())
                            .border_1()
                            .border_color(rgb(BORDER))
                            .when(busy, |d| d.opacity(0.64))
                            .child(restore_icon(busy, -2.))
                            .child(if busy {
                                "Restoring…"
                            } else if error.is_some() {
                                "Retry"
                            } else {
                                "Restore chat"
                            })
                            .on_key_down(cx.listener({
                                let id = restore.clone();
                                move |this, event: &KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        if !busy {
                                            run(this, Action::Restore(id.clone()), window, cx)
                                        }
                                        cx.stop_propagation();
                                    }
                                }
                            }))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if !busy {
                                        run(this, Action::Restore(restore.clone()), window, cx)
                                    }
                                },
                            )),
                        ),
                ),
        )
        .into_any_element()
}
