//! Table text metrics and cells shared by the contribution and session views.
use super::view::{label, row};
use super::*;
use crate::{
    palette::{self, rgb},
    settings_style as style,
    shell::{BG, BORDER, MUTED, TEXT},
};

pub(super) fn tabular() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}
pub(super) struct Columns {
    pub widths: [f32; 4],
    pub minimum: f32,
}
impl Columns {
    pub fn auto(headers: &[String; 4], rows: &[[String; 4]], window: &Window) -> Self {
        let measure = |text: &str, header: bool| {
            let mut font = window.text_style().font();
            font.weight = if header {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            };
            if !header {
                font.features = tabular();
            }
            f32::from(
                window
                    .text_system()
                    .shape_line(
                        text.to_owned().into(),
                        px(12.),
                        &[TextRun {
                            len: text.len(),
                            font,
                            color: window.text_style().color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    )
                    .width(),
            )
        };
        let mut widths: [f32; 4] = std::array::from_fn(|i| measure(&headers[i], true) + 16.);
        for row in rows {
            // The first source cell has max-width:0; its header supplies its preferred width.
            for i in 1..4 {
                widths[i] = widths[i].max(measure(&row[i], false) + 16.);
            }
        }
        let minimum = widths.iter().sum::<f32>().max(1.);
        Self {
            widths: widths.map(|w| w / minimum),
            minimum,
        }
    }
}
pub(super) fn cells(values: [String; 4], widths: [f32; 4], session: bool, last: bool) -> Div {
    let base: Hsla = if session {
        style::card_bg()
    } else {
        rgb(BG).into()
    };
    let hover = base.blend(palette::tone(
        gpui::rgb(0xffffff).opacity(0.02).into(),
        gpui::rgb(0).opacity(0.02).into(),
    ));
    let mut line = row().h(px(28.)).w_full().hover(move |v| v.bg(hover));
    if !last {
        line = line.border_b_1().border_color(rgb(BORDER));
    }
    for (i, text) in values.into_iter().enumerate() {
        line = line.child(
            label(text, 12.)
                .line_height(px(12.))
                .w(relative(widths[i]))
                .flex_none()
                .min_w_0()
                .px(px(8.))
                .truncate()
                .when(i == 0, |v| v.font_weight(FontWeight::MEDIUM))
                .when(i > 0 && (!session || i > 1), |v| v.font_features(tabular()))
                .when(i >= if session { 1 } else { 2 }, |v| {
                    v.text_align(TextAlign::Right)
                })
                .text_color(rgb(if i == 1 || !session && i == 3 {
                    MUTED
                } else {
                    TEXT
                })),
        );
    }
    line
}
pub(super) fn heading(values: [String; 4], widths: [f32; 4]) -> Div {
    let mut line = row()
        .h(px(28.))
        .w_full()
        .border_b_1()
        .border_color(rgb(BORDER));
    for (i, text) in values.into_iter().enumerate() {
        line = line.child(
            label(text, 12.)
                .line_height(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(MUTED))
                .w(relative(widths[i]))
                .flex_none()
                .min_w_0()
                .px(px(8.))
                .truncate()
                .when(i >= 2, |v| v.text_align(TextAlign::Right)),
        );
    }
    line
}
