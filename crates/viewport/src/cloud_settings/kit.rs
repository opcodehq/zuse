//! Shared visual pieces for the Cloud settings pages and the first-run
//! onboarding: status pills, dithered meters and step progress, section cards
//! with inline help, empty states and machine spec chips. Billing and Usage
//! can use the same pieces so every Cloud surface reads as one system.
//!
//! Spacing scale: 4 · 8 · 12 · 16 · 24. Type scale: 10 (caption) · 11
//! (description) · 12 (title) · 15 (step title) · 18 (welcome headline).
use crate::tooltip::TooltipExt as _;
use crate::{
    assets::icon,
    palette, settings_style as s,
    shell::{BORDER, MUTED, TEXT},
};
use gpui::{prelude::*, *};

/// Gap between stacked sections on a page.
pub const SECTION_GAP: f32 = 16.;
/// Horizontal padding inside cards and rows: the shared row inset.
pub const INSET: f32 = s::ROW_PAD_X;

/// The Zuse lime, tuned per theme.
pub fn accent() -> Hsla {
    palette::accent_or(palette::tone(
        hsla(83. / 360., 0.72, 0.46, 1.),
        hsla(83. / 360., 0.74, 0.40, 1.),
    ))
}

/// The label colour on an [`accent`] fill.
pub fn on_accent() -> Hsla {
    hsla(132. / 360., 0.3, 0.07, 1.)
}

/// What a status pill says about the thing it labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Set up and working.
    Ready,
    /// Needs the owner before Cloud can use it.
    Attention,
    /// Working on it: building, verifying, waiting.
    Progress,
    /// Optional, default or informational.
    Neutral,
    /// Broken or unavailable.
    Error,
}

fn tone_colors(tone: Tone) -> (Hsla, Hsla, Hsla) {
    // (background, text, dot)
    match tone {
        Tone::Ready => {
            let ink = palette::tone(
                hsla(133.9 / 360., 0.606, 0.488, 1.),
                hsla(131.4 / 360., 0.734, 0.31, 1.),
            );
            (
                palette::tone(
                    hsla(132.9 / 360., 0.359, 0.153, 1.),
                    hsla(130. / 360., 0.5, 0.953, 1.),
                ),
                ink,
                ink,
            )
        }
        Tone::Attention => {
            let ink = palette::tone(
                hsla(32. / 360., 0.92, 0.6, 1.),
                hsla(26. / 360., 0.9, 0.42, 1.),
            );
            (
                palette::tone(
                    hsla(28. / 360., 0.32, 0.17, 1.),
                    hsla(32. / 360., 1., 0.955, 1.),
                ),
                ink,
                ink,
            )
        }
        Tone::Progress => (accent().opacity(0.14), palette::rgb(TEXT).into(), accent()),
        Tone::Neutral => (
            s::frame_bg().opacity(0.7),
            palette::rgb(MUTED).into(),
            Hsla::from(palette::rgb(MUTED)).opacity(0.6),
        ),
        Tone::Error => {
            let ink = s::destructive_text();
            (
                palette::tone(
                    hsla(349.3 / 360., 0.311, 0.176, 1.),
                    hsla(0., 1., 0.973, 1.),
                ),
                ink,
                ink,
            )
        }
    }
}

/// A rounded status pill: a dot and a short label ("Connected", "Needed").
pub fn pill(label: impl Into<SharedString>, tone: Tone) -> Div {
    let (bg, text, dot) = tone_colors(tone);
    div()
        .flex_none()
        .h(px(18.))
        .pl(px(6.))
        .pr(px(8.))
        .gap(px(5.))
        .rounded_full()
        .flex()
        .items_center()
        .whitespace_nowrap()
        .bg(bg)
        .text_size(px(10.))
        .line_height(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(text)
        .child(div().flex_none().size(px(6.)).rounded_full().bg(dot))
        .child(label.into())
}

/// A dithered meter `height` tall, `fraction` lit in the accent colour.
pub fn meter(fraction: f32, height: f32) -> Div {
    meter_colored(fraction, height, accent())
}

pub fn meter_colored(fraction: f32, height: f32, on: Hsla) -> Div {
    div()
        .relative()
        .w_full()
        .h(px(height))
        .child(crate::dither::meter(
            fraction,
            on,
            Hsla::from(palette::rgb(MUTED)).opacity(0.35),
        ))
}

/// A labelled quantity: title and value on one line, a dithered meter, and a
/// caption under it. Used for plan minutes and anything else with a limit.
pub fn quantity(
    title: impl Into<SharedString>,
    value: impl Into<SharedString>,
    fraction: f32,
    caption: impl Into<SharedString>,
) -> Div {
    let caption = caption.into();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(px(12.))
                .child(s::title_text(title))
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette::rgb(TEXT))
                        .child(value.into()),
                ),
        )
        .child(meter(fraction, 8.))
        .when(!caption.is_empty(), |v| {
            v.child(s::description_text(caption))
        })
}

/// The primary action: the app's dithered key with a crisp label, plus
/// hover, pressed and disabled states.
pub fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    crate::styling::dither_action(id, label, enabled, true).tab_index(if enabled { 0 } else { -1 })
}

/// Where one step of a sequence stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepMark {
    Done,
    Skipped,
    Current,
    Upcoming,
}

/// Segmented step progress: thin solid segments. Done and current steps are
/// the accent, skipped ones a muted fill and upcoming ones a subtle track.
pub fn step_progress(marks: &[StepMark]) -> Div {
    let mut bar = div().flex().items_center().gap(px(4.)).w_full();
    for mark in marks {
        let color: Hsla = match mark {
            StepMark::Done | StepMark::Current => accent(),
            StepMark::Skipped => Hsla::from(palette::rgb(MUTED)).opacity(0.45),
            StepMark::Upcoming => Hsla::from(palette::rgb(BORDER)),
        };
        bar = bar.child(div().flex_1().h(px(3.)).rounded_full().bg(color));
    }
    bar
}

/// A small info glyph that explains `title` on hover or focus. Pages keep
/// short titles; the why lives here instead of in paragraphs.
pub fn info_tip(title: impl Into<SharedString>, text: impl Into<SharedString>) -> Stateful<Div> {
    let title = title.into();
    let text = text.into();
    div()
        .id(SharedString::from(format!("cloud-tip-{title}")))
        .role(Role::Button)
        .aria_label(title)
        .aria_description(text.clone())
        .tab_index(0)
        .size(px(20.))
        // Keep the header at the title's 16px line, like local sections.
        .my(px(-2.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .text_color(palette::rgb(MUTED))
        .hover(|v| v.text_color(palette::rgb(TEXT)))
        .child(icon("info-circle").size(px(12.)))
        .tip(text)
}

/// A title with its explanation behind an [`info_tip`].
pub fn title_with_tip(title: impl Into<SharedString>, tip: impl Into<SharedString>) -> Div {
    let title = title.into();
    let tip = tip.into();
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .min_w_0()
        .child(s::title_text(title.clone()).truncate())
        .when(!tip.is_empty(), |v| v.child(info_tip(title, tip)))
}

/// The floating surface of an overflow ("…") menu.
pub fn menu_surface() -> Div {
    div()
        .flex()
        .flex_col()
        .min_w(px(180.))
        .p(px(4.))
        .rounded(px(9.))
        .bg(palette::tone(hsla(132. / 360., 0.05, 0.105, 1.), white()))
        .border_1()
        .border_color(s::card_border())
        .shadow_lg()
        .occlude()
}

/// One item of an overflow menu. `danger` items use the destructive colour.
pub fn menu_item(
    id: impl Into<ElementId>,
    glyph: &'static str,
    label: impl Into<SharedString>,
    danger: bool,
) -> Stateful<Div> {
    let label = label.into();
    let color: Hsla = if danger {
        s::destructive_text()
    } else {
        palette::rgb(TEXT).into()
    };
    div()
        .id(id)
        .role(Role::MenuItem)
        .aria_label(label.clone())
        .tab_index(0)
        .h(px(28.))
        .px(px(8.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(color)
        .hover(|v| v.bg(s::frame_bg()))
        .child(icon(glyph).size(px(13.)).text_color(color))
        .child(label)
}

/// A titled Cloud section: the shared settings double card
/// ([`s::section`]), the same frame, header and inner card every local
/// settings page uses. The description is help text behind an info glyph
/// next to the title, so pages stay short.
pub fn section(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    action: Option<AnyElement>,
    body: impl IntoElement,
) -> Div {
    s::section(
        s::header_bar(title_with_tip(title, description), action),
        body,
    )
}

/// The hairline between rows and around containers.
pub fn hairline() -> Hsla {
    s::card_border()
}

/// The floating "unsaved changes" bar, centred near the bottom of the
/// window: an info glyph, "Unsaved changes", then the caller's Reset and
/// Save. Callers render it only while there are edits.
pub fn unsaved_bar(
    viewport: Size<Pixels>,
    reset: impl IntoElement,
    save: impl IntoElement,
) -> impl IntoElement {
    const WIDTH: f32 = 360.;
    const HEIGHT: f32 = 44.;
    let x = (f32::from(viewport.width) - WIDTH) / 2.;
    let y = f32::from(viewport.height) - HEIGHT - 24.;
    deferred(
        anchored()
            .position_mode(AnchoredPositionMode::Window)
            .position(point(px(x.max(8.)), px(y.max(8.))))
            .child(
                div()
                    .id("unsaved-changes-bar")
                    .role(Role::Status)
                    .aria_label("Unsaved changes")
                    .occlude()
                    .w(px(WIDTH))
                    .h(px(HEIGHT))
                    .pl(px(14.))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(hairline())
                    .bg(palette::tone(hsla(0., 0., 0.13, 1.), hsla(0., 0., 1., 1.)))
                    .shadow_lg()
                    .child(
                        icon("info-circle")
                            .size(px(14.))
                            .text_color(palette::rgb(MUTED)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette::rgb(TEXT))
                            .child("Unsaved changes"),
                    )
                    .child(reset)
                    .child(save),
            ),
    )
    .with_priority(2)
}

/// A quiet, destructive-tinted text button ("Reset").
pub fn reset_button(id: impl Into<ElementId>, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label("Reset")
        .tab_index(if enabled { 0 } else { -1 })
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(7.))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(s::destructive_text())
        .when(!enabled, |v| v.opacity(0.5))
        .when(enabled, |v| {
            v.cursor_pointer()
                .hover(|v| v.bg(s::destructive_text().opacity(0.1)))
        })
        .child("Reset")
}

/// A quiet text button ("Back", "Skip for now").
pub fn quiet_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette::rgb(MUTED))
        .hover(|v| {
            v.text_color(palette::rgb(TEXT))
                .bg(s::frame_bg().opacity(0.6))
        })
        .child(label)
}

/// A rounded square holding one glyph, for row and step leaders.
pub fn icon_tile(glyph: &'static str, size: f32) -> Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded(px((size / 4.).round()))
        .bg(s::frame_bg())
        .border_1()
        .border_color(palette::rgb(BORDER).opacity(0.6))
        .flex()
        .items_center()
        .justify_center()
        .child(
            icon(glyph)
                .size(px((size / 2.).round()))
                .text_color(palette::rgb(TEXT)),
        )
}

/// A centred empty state inside a card: tile, title, one sentence and the
/// action that fills it.
pub fn empty_state(
    glyph: &'static str,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    action: Option<AnyElement>,
) -> Div {
    let body: SharedString = body.into();
    div()
        .flex()
        .flex_col()
        .items_center()
        .px(px(24.))
        .py(px(24.))
        .gap(px(8.))
        .child(icon_tile(glyph, 36.))
        .child(div().mt(px(4.)).child(s::title_text(title)))
        .when(!body.is_empty(), |v| {
            v.child(
                div()
                    .max_w(px(360.))
                    .text_center()
                    .child(s::description_text(body)),
            )
        })
        .when_some(action, |v, action| v.child(div().mt(px(8.)).child(action)))
}

/// A row of a section's inner card ([`s::row`]): leading element, title and
/// description, trailing element.
pub fn row_item(
    first: bool,
    leading: Option<AnyElement>,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    trailing: Option<AnyElement>,
) -> Div {
    let description = description.into();
    s::row(first)
        .flex_nowrap()
        .when_some(leading, |v, leading| v.child(leading))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(s::title_text(title).truncate())
                .when(!description.is_empty(), |v| {
                    v.child(s::description_text(description))
                }),
        )
        .when_some(trailing, |v, trailing| {
            v.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(trailing),
            )
        })
}

/// A monospace chip for a machine size, e.g. "4 vCPU · 16 GB".
pub fn spec_chip(label: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .h(px(22.))
        .px(px(8.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .border_1()
        .border_color(palette::rgb(BORDER))
        .bg(s::frame_bg().opacity(0.6))
        .font_family("Geist Mono")
        .text_size(px(11.))
        .text_color(palette::rgb(TEXT))
        .child(label.into())
}

/// One option of a size picker. `locked` options show why in `note`.
pub struct Choice {
    pub title: SharedString,
    pub detail: SharedString,
    pub selected: bool,
    pub locked: Option<SharedString>,
}

/// Side-by-side option cards. Selection is shown, never hidden; a locked
/// option names the plan that unlocks it.
pub fn choice_cards(choices: Vec<Choice>) -> Div {
    let mut row = div().flex().gap(px(8.)).w_full();
    for choice in choices {
        let locked = choice.locked.is_some();
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(INSET))
                .rounded(px(10.))
                .border_1()
                .border_color(if choice.selected {
                    accent()
                } else {
                    palette::rgb(BORDER).into()
                })
                .when(choice.selected, |v| v.bg(accent().opacity(0.06)))
                .when(locked, |v| v.opacity(0.7))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_family("Geist Mono")
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette::rgb(TEXT))
                                .child(choice.title),
                        )
                        .when(choice.selected, |v| {
                            v.child(icon("check").size(px(12.)).text_color(accent()))
                        })
                        .when_some(choice.locked.clone(), |v, note| {
                            v.child(pill(note, Tone::Neutral))
                        }),
                )
                .when(!choice.detail.is_empty(), |v| {
                    v.child(s::description_text(choice.detail))
                }),
        );
    }
    row
}

/// A quiet line of help with an info glyph, under a control or section.
pub fn help(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(6.))
        .child(
            icon("info-circle")
                .mt(px(2.))
                .size(px(12.))
                .text_color(palette::rgb(MUTED)),
        )
        .child(s::description_text(text))
}

/// Formats a count with thousands separators: 33600 → "33,600".
pub fn thousands(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if value < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::thousands;

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(8_400), "8,400");
        assert_eq!(thousands(33_600), "33,600");
        assert_eq!(thousands(1_084_000), "1,084,000");
        assert_eq!(thousands(-1200), "-1,200");
    }
}
