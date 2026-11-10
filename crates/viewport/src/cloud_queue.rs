//! The first Cloud prompt stays visible above its independently editable draft.
use gpui::{prelude::*, *};
use serde_json::Value;

pub fn background() -> Hsla {
    crate::palette::tone(gpui::rgba(0x181916b8).into(), gpui::rgba(0xffffffb8).into())
}

#[derive(Clone)]
pub struct Prompt {
    text: String,
    attachments: Vec<String>,
}

impl Prompt {
    pub(crate) fn display_text(&self) -> String {
        if self.attachments.is_empty() {
            return self.text.clone();
        }
        format!(
            "{}\n\nAttachments: {}",
            self.text,
            self.attachments.join(", ")
        )
    }
    pub fn from_request(request: &Value) -> Self {
        Self {
            text: request["firstMessage"].as_str().unwrap_or_default().into(),
            attachments: request["attachmentTransfers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file["fileName"].as_str().map(str::to_owned))
                .collect(),
        }
    }
}

pub fn queue(
    prompt: Prompt,
    status: String,
    busy: bool,
    animate: bool,
    text: Hsla,
    muted: Hsla,
    border: Hsla,
) -> Div {
    let spinner = crate::assets::icon("loading")
        .size(px(12.))
        .text_color(muted);
    let spinner = if animate {
        spinner
            .with_animation(
                "cloud-queue-spinner",
                Animation::new(std::time::Duration::from_secs(1)).repeat(),
                |glyph, progress| {
                    glyph.with_transformation(Transformation::rotate(percentage(progress)))
                },
            )
            .into_any_element()
    } else {
        spinner.into_any_element()
    };
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id("cloud-queue-status")
                .role(Role::Status)
                .aria_label(status.clone())
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(6.))
                .border_b_1()
                .border_color(border)
                .text_size(px(11.))
                .line_height(px(14.85))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .when(busy, |row| {
                    row.child(div().flex_none().size(px(12.)).child(spinner))
                })
                .child(status_label(status, busy && animate, muted)),
        )
        .child(
            div()
                .id("cloud-queued-prompt")
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .px(px(12.))
                .py(px(8.))
                .border_b_1()
                .border_color(border)
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(text)
                .child(div().min_w_0().line_clamp(3).child(prompt.text))
                .when(!prompt.attachments.is_empty(), |row| {
                    row.child(
                        div()
                            .truncate()
                            .text_color(muted)
                            .child(prompt.attachments.join(", ")),
                    )
                }),
        )
}

fn status_label(status: String, animate: bool, muted: Hsla) -> AnyElement {
    let label = div().min_w_0().flex_1();
    if !animate {
        return label.child(status).into_any_element();
    }
    let base: Rgba = muted.into();
    label
        .id("cloud-queue-label")
        .with_animation(
            "cloud-queue-shimmer",
            Animation::new(std::time::Duration::from_millis(1850)).repeat(),
            move |label, progress| {
                // Sweep for 1.5 seconds, then rest for 350ms.
                let t = (progress * 1850. / 1500.).min(1.);
                let center = -0.2 + 1.4 * t * t * (3. - 2. * t);
                let length = status.chars().count().max(1) as f32;
                let highlights = status
                    .char_indices()
                    .enumerate()
                    .map(|(index, (offset, ch))| {
                        let strength =
                            (1. - (index as f32 / length - center).abs() / 0.18).clamp(0., 1.);
                        let color = Rgba {
                            r: base.r + (0.81048 - base.r) * strength,
                            g: base.g + (0.9908 - base.g) * strength,
                            b: base.b + (0.0892 - base.b) * strength,
                            a: base.a,
                        };
                        (
                            offset..offset + ch.len_utf8(),
                            HighlightStyle {
                                color: Some(color.into()),
                                ..Default::default()
                            },
                        )
                    });
                label.child(StyledText::new(status.clone()).with_highlights(highlights))
            },
        )
        .into_any_element()
}
