//! Form pieces shared by the Environments and Machine pages: titled sections
//! whose descriptions stay visible, bordered script fields and a save bar.
use super::*;

/// A titled card. Unlike `cs::group`, the description is always visible.
pub(super) fn section(
    title: &'static str,
    description: &'static str,
    action: Option<AnyElement>,
    body: impl IntoElement,
) -> Div {
    let header = row()
        .items_start()
        .justify_between()
        .gap(px(12.))
        .px(px(12.))
        .pt(px(10.))
        .pb(px(10.))
        .child(
            column()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(s::title_text(title))
                .when(!description.is_empty(), |v| {
                    v.child(s::description_text(description))
                }),
        )
        .when_some(action, |v, action| {
            v.child(
                div()
                    .flex_none()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .child(action),
            )
        });
    s::frame().child(header).child(s::card().child(body))
}

/// Label, hint and a bordered script editor.
pub(super) fn script_field(
    label: &'static str,
    hint: &'static str,
    input: Entity<TextInput>,
) -> Div {
    column()
        .gap(px(6.))
        .child(
            row()
                .items_baseline()
                .gap(px(8.))
                .child(s::title_text(label))
                .child(s::description_text(hint)),
        )
        .child(script_box(input))
}

pub(super) fn script_box(input: Entity<TextInput>) -> Div {
    div()
        .w_full()
        .min_h(px(72.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(7.))
        .border_1()
        .border_color(palette::rgb(BORDER))
        .bg(palette::tone(
            hsla(0., 0., 0.06, 1.),
            hsla(0., 0., 0.98, 1.),
        ))
        .child(input)
}

/// A one-line bordered input that takes the space it is given.
pub(super) fn line_field(input: Entity<TextInput>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .h(px(30.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(7.))
        .border_1()
        .border_color(palette::rgb(BORDER))
        .bg(palette::tone(
            hsla(0., 0., 0.06, 1.),
            hsla(0., 0., 0.98, 1.),
        ))
        .child(input)
}

/// Quiet copy for an empty list inside a section.
pub(super) fn empty(text: &'static str) -> Div {
    div()
        .px(px(12.))
        .py(px(14.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(palette::rgb(MUTED))
        .child(text)
}

pub(super) fn mono(text: impl Into<SharedString>) -> Div {
    div()
        .font_family("Geist Mono")
        .text_size(px(11.5))
        .line_height(px(16.))
        .text_color(palette::rgb(MUTED))
        .truncate()
        .child(text.into())
}

pub(super) fn message(text: &str, error: bool) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(if error {
            s::destructive_text()
        } else {
            palette::rgb(MUTED).into()
        })
        .child(text.to_owned())
}

/// Status on the left; Discard and Save on the right, live only when dirty.
pub(super) fn save_bar(
    id: &'static str,
    status: String,
    dirty: bool,
    busy: bool,
    discard: impl Fn(&mut RepositoryPanel, &mut Context<RepositoryPanel>) + 'static,
    save: impl Fn(&mut RepositoryPanel, &mut Context<RepositoryPanel>) + 'static,
    cx: &mut Context<RepositoryPanel>,
) -> Div {
    let ready = dirty && !busy;
    row()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(palette::rgb(BORDER))
        .bg(s::frame_bg())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette::rgb(if dirty { TEXT } else { MUTED }))
                .child(if dirty {
                    "Unsaved changes".to_owned()
                } else {
                    status
                }),
        )
        .child(
            row()
                .gap(px(8.))
                .when(dirty, |v| {
                    v.child(
                        cs::button(
                            SharedString::from(format!("{id}-discard")),
                            "Discard",
                            false,
                            busy,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| discard(this, cx))),
                    )
                })
                .child(
                    crate::styling::dither_action(
                        SharedString::from(format!("{id}-save")),
                        if busy { "Saving…" } else { "Save changes" },
                        ready,
                        true,
                    )
                    .tab_index(if ready { 0 } else { -1 })
                    .when(ready, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| save(this, cx)))
                    }),
                ),
        )
}
