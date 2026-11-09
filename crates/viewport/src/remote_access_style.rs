//! Compact remote access cards and focused connection dialogs shared by both hosts.
use crate::localization::message as tr;
use crate::{
    assets::icon,
    palette::rgb,
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use zuse_i18n::messages as msg;

pub fn glyph(name: &str) -> Svg {
    icon(name)
}

pub fn frame(
    title: impl Into<SharedString>,
    help: impl Into<SharedString>,
    action: Option<AnyElement>,
) -> Div {
    let title = title.into();
    let access_header = title.as_ref() == "Remote access to this computer";
    let help = help.into();
    s::frame().child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(12.))
            .pt(px(if access_header { 6. } else { 10. }))
            .pb(px(if access_header { 8. } else { 10. }))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child(title.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("remote-help-{title}")))
                            .role(Role::Button)
                            .aria_label(format!("About {title}"))
                            .tab_index(0)
                            .size(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .text_color(rgb(MUTED).opacity(0.6))
                            .hover(|v| v.bg(s::frame_bg()).text_color(rgb(MUTED)))
                            .tooltip(move |_, cx| cx.new(|_| Help(help.clone())).into())
                            .child(icon("info-circle").size(px(14.))),
                    ),
            )
            .children(action),
    )
}

struct Help(SharedString);
impl Render for Help {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(256.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(s::card_border())
            .bg(s::card_bg())
            .text_color(rgb(TEXT))
            .text_size(px(12.))
            .line_height(px(16.))
            .shadow_sm()
            .child(self.0.clone())
    }
}

pub fn method_row(
    glyph: &str,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    first: bool,
) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .min_h(px(if first { 56. } else { 57. }))
        .gap(px(10.))
        .px(px(12.))
        .py(px(8.))
        .when(!first, |v| {
            v.border_t_1().border_color(s::card_border().opacity(0.4))
        })
        .child(
            div()
                .size(px(28.))
                .flex_none()
                .rounded(px(6.))
                .bg(s::frame_bg())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph).size(px(16.)).text_color(rgb(MUTED))),
        )
        .child(details(title, description).flex_1())
}

pub fn details(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .truncate()
                .text_size(px(12.))
                .line_height(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT))
                .child(title.into()),
        )
        .child(
            div()
                .truncate()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(rgb(MUTED))
                .child(description.into()),
        )
}

pub fn button(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Stateful<Div> {
    s::button(id, title)
        .tab_index(0)
        .px(px(15.))
        .rounded(px(6.))
        .text_size(px(12.))
}

pub fn small_button(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Stateful<Div> {
    button(id, title).h(px(24.)).px(px(11.)).text_size(px(10.))
}
pub fn method_button(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    _cx: &gpui::App,
) -> Stateful<Div> {
    small_button(id, title).h(px(28.))
}
pub fn add_computer_button(id: impl Into<ElementId>) -> Stateful<Div> {
    small_button(id, "")
        .px(px(13.))
        .text_size(px(11.))
        .child(icon("plus").size(px(14.)))
        .child(crate::localization::element(
            msg::chat::computer_switcher_add_computer,
        ))
}

pub fn connect_row(ready: bool, cx: &gpui::App) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .min_h(px(56.))
        .gap(px(10.))
        .px(px(12.))
        .py(px(8.))
        .child(
            div()
                .size(px(28.))
                .flex_none()
                .rounded(px(6.))
                .bg(s::frame_bg())
                .flex()
                .items_center()
                .justify_center()
                .child(icon("link").size(px(16.))),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(tr(
                            cx,
                            msg::settings::connect_link_card_create_a_connect_link,
                        )),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(rgb(MUTED))
                        .child(if ready {
                            ("Authorize another device with a link that lasts ten minutes.").to_string()
                        } else {
                            crate::localization::text("settings-connect_link_card_set_up_one_of_the_connections_above_first", cx)
                        }),
                ),
        )
}

pub fn empty_computers(cx: &gpui::App) -> Div {
    div()
        .flex()
        .items_center()
        .min_h(px(80.))
        .gap(px(12.))
        .px(px(12.))
        .py(px(12.))
        .child(
            div()
                .size(px(32.))
                .flex_none()
                .rounded(px(6.))
                .bg(s::frame_bg())
                .flex()
                .items_center()
                .justify_center()
                .child(icon("monitor").size(px(16.))),
        )
        .child(details(
            tr(cx, msg::native::no_other_computers_yet),
            tr(cx, msg::settings::using_computers_card_add_computer_help),
        ))
}

pub fn empty(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(12.))
        .py(px(16.))
        .text_size(px(11.))
        .line_height(px(14.85))
        .text_color(rgb(MUTED))
        .child(text.into())
}

pub fn dialog_panel(
    id: &'static str,
    title: &'static str,
    description: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Dialog)
        .aria_label(title)
        .tab_group()
        .tab_stop(false)
        .flex_none()
        .w(px(448.))
        .max_w(relative(1.))
        .max_h(relative(0.85))
        .overflow_y_scroll()
        .rounded(px(12.))
        .border_1()
        .border_color(s::card_border())
        .bg(s::card_bg())
        .text_color(rgb(TEXT))
        .flex()
        .flex_col()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(15.))
                        .line_height(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .text_color(rgb(MUTED))
                        .child(description),
                ),
        )
}

pub fn dialog_body() -> Div {
    div()
        .px(px(16.))
        .pb(px(16.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .min_w_0()
}
pub fn dialog_footer() -> Div {
    div()
        .border_t_1()
        .border_color(s::card_border())
        .bg(s::frame_bg())
        .px(px(16.))
        .py(px(10.))
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.))
}

#[derive(IntoElement)]
pub struct Modal(pub AnyElement);
impl RenderOnce for Modal {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let viewport = window.viewport_size();
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(px(0.), px(0.)))
                .child(
                    div()
                        .id("remote-access-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .p(px(16.))
                        .bg(black().opacity(0.32))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(self.0),
                ),
        )
        .priority(3)
    }
}
