//! Context occupancy is read from the replicated chat snapshot, never local CLI state.
//!
//! The composer's usage indicator: one ring button in the toolbar (between
//! the model pill and the send key) whose glass card stacks the context
//! window section above the plan-usage section ([`crate::account_usage`]).
use crate::localization::message as tr;
use crate::theme::Theme;
use gpui::{SharedString, div, prelude::*, px};
use zuse_i18n::messages as msg;
use zuse_protocol::ContextUsage;

/// Usage card geometry: `w-[300px]`, `p-3.5` sections with `gap-3` rows.
pub(crate) const CARD_WIDTH: f32 = 300.0;
const SECTION_PAD: f32 = 14.0;
const SECTION_GAP: f32 = 12.0;
/// The segmented meter: thin rounded sticks with a 2px gutter.
const METER_SEGMENTS: usize = 36;
const METER_HEIGHT: f32 = 16.0;
const METER_GAP: f32 = 2.0;
/// The ring button matches the toolbar's other 28px icon buttons.
const RING_BUTTON_SIZE: f32 = 28.0;
const RING_DIAMETER: f32 = 14.0;
/// At or past this share of the window the ring and meter turn amber.
const HIGH_FRACTION: f64 = 0.9;

/// whether the indicator has anything to measure against: harnesses that
/// never report a window (antigravity) get no context section at all, rather
/// than a permanently empty meter.
pub fn has_window(usage: Option<ContextUsage>) -> bool {
    usage
        .and_then(|usage| usage.window)
        .is_some_and(|window| window > 0)
}

/// Compact token counts: `999`, `241.4K`, `1M`, `1.2M`. Pure.
pub(crate) fn format_tokens(count: u64) -> String {
    fn compact(value: f64, suffix: &str) -> String {
        let rounded = (value * 10.0).round() / 10.0;
        if rounded.fract() == 0.0 {
            format!("{rounded:.0}{suffix}")
        } else {
            format!("{rounded:.1}{suffix}")
        }
    }
    match count {
        0..=999 => count.to_string(),
        // 999,950+ would round up to "1000K".
        1_000..=999_949 => compact(count as f64 / 1_000.0, "K"),
        _ => compact(count as f64 / 1_000_000.0, "M"),
    }
}

/// The context section's readings, when a window is reported. Pure.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContextReading {
    /// `241.4K / 1M`, or just the window while no turn has reported tokens.
    pub header: String,
    /// Share of the window in use, 0..=1; `None` until tokens are reported.
    pub fraction: Option<f64>,
    pub available: Option<String>,
}

pub(crate) fn reading(usage: Option<ContextUsage>) -> Option<ContextReading> {
    let usage = usage?;
    let window = usage.window.filter(|window| *window > 0)?;
    Some(match usage.tokens {
        Some(tokens) => ContextReading {
            header: format!("{} / {}", format_tokens(tokens), format_tokens(window)),
            fraction: Some((tokens as f64 / window as f64).clamp(0.0, 1.0)),
            available: Some(format_tokens(window.saturating_sub(tokens))),
        },
        None => ContextReading {
            header: format_tokens(window),
            fraction: None,
            available: None,
        },
    })
}

/// The toolbar's ring button: a 14px progress ring in a quiet 28px key.
/// `open` holds the hover wash while the card is up; `high` tints amber.
pub(crate) fn ring_button(
    fraction: Option<f32>,
    high: bool,
    open: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let color = if high {
        theme.warning
    } else {
        theme.text_muted
    };
    div()
        .id("composer-usage")
        .flex_none()
        .size(px(RING_BUTTON_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.0))
        .cursor_pointer()
        .when(open, |s| s.bg(theme.muted_fill().opacity(0.6)))
        .hover(|s| s.bg(theme.muted_fill().opacity(0.6)))
        .child(zuse_viewport::context_status::progress_ring(
            fraction,
            color,
            color.opacity(0.25),
            RING_DIAMETER,
        ))
}

/// One usage card section: `p-3.5` with `gap-3` between its rows.
pub(crate) fn section(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(SECTION_GAP))
        .p(px(SECTION_PAD))
        .text_size(crate::typography::text_sm())
        .text_color(theme.text)
}

/// `label … value` on one line; the value never wraps.
pub(crate) fn value_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    color: gpui::Hsla,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .text_color(color)
        .child(div().min_w_0().truncate().child(label.into()))
        .child(div().flex_none().child(value.into()))
}

/// A section's title row: medium label, muted value.
pub(crate) fn title_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    theme: &Theme,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label.into()),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme.text_muted)
                .child(value.into()),
        )
}

/// How many of the meter's sticks a fraction lights. Any usage at all lights
/// at least one, so a fresh window never reads as unreported. Pure.
pub(crate) fn lit_segments(fraction: f64) -> usize {
    (fraction.clamp(0.0, 1.0) * METER_SEGMENTS as f64).ceil() as usize
}

/// The segmented meter: thin rounded sticks, accent-lit up to `fraction`,
/// amber when `high`.
pub(crate) fn stick_meter(fraction: f64, high: bool, theme: &Theme) -> gpui::Div {
    let lit = lit_segments(fraction);
    let on = if high {
        theme.warning.opacity(0.65)
    } else {
        theme.accent.opacity(0.7)
    };
    let off = theme.text_muted.opacity(0.16);
    div()
        .h(px(METER_HEIGHT))
        .flex()
        .gap(px(METER_GAP))
        .children((0..METER_SEGMENTS).map(|ix| {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .rounded(px(2.0))
                .bg(if ix < lit { on } else { off })
        }))
}

/// The card's context section, when the harness reports a window.
pub(crate) fn context_section(
    usage: Option<ContextUsage>,
    theme: &Theme,
    cx: &gpui::App,
) -> Option<gpui::Div> {
    let reading = reading(usage)?;
    let section = section(theme).child(title_row("Context", reading.header, theme));
    Some(match reading.fraction {
        Some(fraction) => section
            .child(stick_meter(fraction, fraction >= HIGH_FRACTION, theme))
            .child(value_row(
                "Window used",
                format!("{:.1}%", fraction * 100.0),
                theme.text_muted,
            ))
            .child(value_row(
                "Available",
                reading.available.unwrap_or_default(),
                theme.text_muted.opacity(0.7),
            )),
        None => section.child(div().text_color(theme.text_muted.opacity(0.7)).child(tr(
            cx,
            msg::chat::chat_composer_usage_appears_after_the_first_response,
        ))),
    })
}

/// Whether the context reading alone warrants the amber ring.
pub(crate) fn is_high(usage: Option<ContextUsage>) -> bool {
    reading(usage)
        .and_then(|reading| reading.fraction)
        .is_some_and(|fraction| fraction >= HIGH_FRACTION)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indicator_needs_a_reported_window() {
        assert!(!has_window(None));
        assert!(!has_window(Some(ContextUsage {
            tokens: Some(1_200),
            window: None,
        })));
        assert!(!has_window(Some(ContextUsage {
            tokens: Some(1_200),
            window: Some(0),
        })));
        assert!(has_window(Some(ContextUsage {
            tokens: None,
            window: Some(200_000),
        })));
    }

    #[test]
    fn token_counts_are_compact() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(5_417), "5.4K");
        assert_eq!(format_tokens(200_000), "200K");
        assert_eq!(format_tokens(241_400), "241.4K");
        assert_eq!(format_tokens(999_960), "1M");
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(1_048_576), "1M");
        assert_eq!(format_tokens(1_250_000), "1.3M");
    }

    #[test]
    fn reading_matches_the_card() {
        let reading = reading(Some(ContextUsage {
            tokens: Some(241_400),
            window: Some(1_000_000),
        }))
        .unwrap();
        assert_eq!(reading.header, "241.4K / 1M");
        assert_eq!(reading.available.as_deref(), Some("758.6K"));
        assert_eq!(
            format!("{:.1}%", reading.fraction.unwrap() * 100.0),
            "24.1%"
        );
    }

    #[test]
    fn missing_usage_is_distinct_from_zero_and_overflow() {
        assert_eq!(reading(None), None);
        assert_eq!(
            reading(Some(ContextUsage {
                tokens: Some(10),
                window: Some(0)
            })),
            None
        );
        let waiting = reading(Some(ContextUsage {
            tokens: None,
            window: Some(200_000),
        }))
        .unwrap();
        assert_eq!((waiting.header.as_str(), waiting.fraction), ("200K", None));
        let zero = reading(Some(ContextUsage {
            tokens: Some(0),
            window: Some(200),
        }))
        .unwrap();
        assert_eq!(zero.fraction, Some(0.0));
        assert_eq!(zero.available.as_deref(), Some("200"));
        let over = reading(Some(ContextUsage {
            tokens: Some(250),
            window: Some(200),
        }))
        .unwrap();
        assert_eq!(over.fraction, Some(1.0));
        assert_eq!(over.available.as_deref(), Some("0"));
    }

    #[test]
    fn meter_lights_any_usage() {
        assert_eq!(lit_segments(0.0), 0);
        assert_eq!(lit_segments(0.001), 1);
        assert_eq!(lit_segments(0.241), 9);
        assert_eq!(lit_segments(1.4), METER_SEGMENTS);
    }
}
