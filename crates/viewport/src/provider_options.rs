//! Shared disclosure for the secondary controls in provider settings.
use crate::{
    assets::icon,
    palette::{rgb, tone},
    settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use zuse_protocol::HarnessUpdatePhase as Phase;

pub fn header(
    id: impl Into<ElementId>,
    expanded: bool,
    phase: Option<Phase>,
    completion: bool,
    cx: &gpui::App,
) -> Stateful<Div> {
    let (caption, attention) = match phase {
        Some(Phase::Available) => ("An update is available", true),
        Some(Phase::Failed) => ("An update needs attention", true),
        Some(Phase::ManualActionRequired) => ("An update needs manual action", true),
        Some(Phase::WaitingForIdle) => ("Update waiting for this agent to finish", true),
        Some(Phase::Preparing | Phase::Downloading | Phase::Installing | Phase::Verifying) => {
            ("Agent update in progress", true)
        }
        Some(Phase::Checking) => ("Checking for updates…", false),
        _ if completion => ("Completion and update preferences", false),
        _ => ("CLI updates and update policy", false),
    };
    let color = if phase == Some(Phase::Failed) {
        s::destructive_text()
    } else if attention {
        tone(rgb(0xfbbf24).into(), rgb(0xb45309).into())
    } else {
        rgb(MUTED).into()
    };
    s::button(id, "")
        .aria_label(crate::localization::message(
            cx,
            zuse_i18n::messages::native::agent_options,
        ))
        .tab_index(0)
        .w_full()
        .h(px(40.))
        .px(px(8.))
        .gap(px(8.))
        .justify_start()
        .border_0()
        .bg(transparent_black())
        .focus_visible(|v| v.bg(s::frame_bg()))
        .child(
            icon(if expanded {
                "alt-arrow-down"
            } else {
                "alt-arrow-right"
            })
            .size(px(14.))
            .flex_none()
            .text_color(rgb(MUTED)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT))
                        .child("Agent options"),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .line_height(px(14.85))
                        .text_color(color)
                        .child(caption),
                ),
        )
}
