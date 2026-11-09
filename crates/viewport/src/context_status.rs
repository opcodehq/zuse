//! Read-only context occupancy from the selected execution host's transcript.
use super::*;
use crate::localization::message as tr;
use crate::shell::{BORDER, FAINT, MUTED, TEXT, column, row};
use zuse_i18n::messages as msg;
use zuse_protocol::ContextUsage;

/// Paint a bounded progress ring without allocating an image or an animation loop.
pub fn progress_ring(fraction: Option<f32>, color: Hsla, track: Hsla, diameter: f32) -> AnyElement {
    let fraction = fraction.unwrap_or(0.).clamp(0., 1.);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let side = diameter.round() as usize;
            for y in 0..side {
                for x in 0..side {
                    let cell = match ring_cell(x, y, side, fraction) {
                        Some(true) => color,
                        Some(false) if crate::dither::threshold(x, y) < 0.5 => track,
                        _ => continue,
                    };
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            bounds.origin + point(px(x as f32), px(y as f32)),
                            gpui::size(px(1.), px(1.)),
                        ),
                        cell,
                    ));
                }
            }
        },
    )
    .size(px(diameter))
    .into_any_element()
}

/// Whether pixel `(x, y)` of a `side`-wide ring is on the band, and if so
/// whether the reading (clockwise from twelve o'clock) has reached it. Pure.
fn ring_cell(x: usize, y: usize, side: usize, fraction: f32) -> Option<bool> {
    let radius = side as f32 / 2.;
    let (dx, dy) = (x as f32 + 0.5 - radius, y as f32 + 0.5 - radius);
    let distance = dx.hypot(dy);
    if distance > radius || distance < radius * 0.56 {
        return None;
    }
    let turn = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    Some(turn <= fraction)
}

fn tokens(value: u64) -> String {
    let (scale, suffix) = if value >= 1_000_000 {
        (1_000_000., "m")
    } else if value >= 1_000 {
        (1_000., "k")
    } else {
        return value.to_string();
    };
    let value = format!("{:.1}", value as f64 / scale);
    format!("{}{suffix}", value.strip_suffix(".0").unwrap_or(&value))
}

impl WorkspaceView {
    pub(super) fn reset_context_status(&mut self) {
        self.context_usage = None;
        self.context_menu = false;
        self.context_confirmed = false;
    }
    fn context_has_window(&self) -> bool {
        self.selected.is_some()
            && self
                .context_usage
                .and_then(|usage| usage.window)
                .is_some_and(|window| window > 0)
    }
    pub(super) fn context_control(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.context_has_window() {
            return None;
        }
        let fraction = self.context_usage.and_then(ContextUsage::fraction);
        let color: Hsla = rgb(if fraction.is_some_and(|value| value >= 0.9) {
            0xfbbf24
        } else {
            MUTED
        })
        .into();
        let anchor = self.context_trigger.clone();
        let trigger = row()
            .id("context-status-trigger")
            .relative()
            .role(Role::Button)
            .aria_label(tr(cx, msg::chat::chat_composer_context_and_usage_status))
            .aria_expanded(self.context_menu)
            .tab_index(0)
            .track_focus(&self.context_focus)
            .h(px(24.))
            .px(px(8.))
            .rounded(px(6.))
            .cursor_pointer()
            .when(self.context_menu, |view| view.bg(rgb(0x202020)))
            .hover(|view| view.bg(rgb(0x202020)))
            .when(!self.context_confirmed || !self.connected, |view| {
                view.opacity(0.6)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(progress_ring(
                fraction.map(|value| value as f32),
                color,
                color.opacity(0.25),
                14.,
            ))
            .on_click(cx.listener(|this, _, window, cx| {
                this.context_menu = !this.context_menu;
                this.model_picker = false;
                this.agent_picker = false;
                this.access_picker = false;
                this.add_menu = false;
                window.focus(&this.context_focus, cx);
                cx.notify();
            }))
            .child(
                canvas(
                    move |bounds, _, _| anchor.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        Some(
            div()
                .relative()
                .flex_none()
                .child(trigger)
                .when_some(self.context_popup(cx), |view, popup| {
                    view.child(deferred(popup).priority(1))
                })
                .into_any_element(),
        )
    }
    pub(super) fn context_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.context_menu || !self.context_has_window() {
            return None;
        }
        let usage = self.context_usage.unwrap();
        let window = usage.window.unwrap();
        let fraction = usage.fraction();
        let high = fraction.is_some_and(|value| value >= 0.9);
        let color = if high {
            Hsla::from(gpui::rgb(0xfcd34d)).opacity(0.65)
        } else {
            hsla(83. / 360., 1., 0.5, 0.7)
        };
        let mut card = column()
            .id("context-status-card")
            .role(Role::Group)
            .aria_label(tr(cx, msg::native::context_window_usage))
            .absolute()
            .bottom_full()
            .mb_2()
            .right_0()
            .w(px(300f32.min(self.content_width - 32.).max(220.)))
            .p(px(14.))
            .gap_3()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(crate::palette::tone(
                hsla(132. / 360., 0.05, 0.105, 1.),
                hsla(0., 0., 1., 1.),
            ))
            .shadow_lg()
            .text_size(px(13.))
            .text_color(rgb(MUTED))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                row()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child(tr(cx, msg::native::context)),
                    )
                    .child(
                        usage
                            .tokens
                            .map(|used| format!("{} / {}", tokens(used), tokens(window)))
                            .unwrap_or_else(|| tokens(window)),
                    ),
            );
        if !self.connected || !self.context_confirmed {
            card = card.child(div().text_size(px(11.)).text_color(rgb(FAINT)).child(
                if self.connected && !self.transcript_refresh_pending() {
                    "Last reported · refresh needed"
                } else if self.connected {
                    "Last reported · refreshing from host"
                } else {
                    "Last reported · offline"
                },
            ));
        }
        if let Some(used) = usage.tokens {
            let fraction = fraction.unwrap_or(0.).clamp(0., 1.);
            let filled = (fraction * 38.).ceil() as usize;
            card = card
                .child(row().w_full().gap(px(2.)).children((0..38).map(|index| {
                    div()
                        .flex_1()
                        .h(px(16.))
                        .rounded(px(2.))
                        .bg(if index < filled {
                            color
                        } else {
                            Hsla::from(rgb(MUTED)).opacity(0.16)
                        })
                })))
                .child(
                    row()
                        .justify_between()
                        .child(tr(cx, msg::native::window_used))
                        .child(format!("{:.1}%", fraction * 100.)),
                )
                .child(
                    row()
                        .justify_between()
                        .text_color(rgb(FAINT))
                        .child(tr(cx, msg::chat::chat_composer_available))
                        .child(tokens(window.saturating_sub(used))),
                );
        } else {
            card = card.child(
                div()
                    .text_color(rgb(FAINT))
                    .child(tr(cx, msg::native::usage_appears_after_the_first_response)),
            );
        }
        Some(
            card.on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if !this
                    .context_trigger
                    .get()
                    .is_some_and(|bounds| bounds.contains(&event.position))
                {
                    this.context_menu = false;
                    cx.notify();
                }
            }))
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod ring_tests {
    use super::ring_cell;

    #[test]
    fn ring_fills_clockwise_and_leaves_the_center_open() {
        assert_eq!(ring_cell(7, 7, 14, 1.0), None);
        // Right of center is a quarter turn around.
        assert_eq!(ring_cell(12, 7, 14, 0.2), Some(false));
        assert_eq!(ring_cell(12, 7, 14, 0.3), Some(true));
        // Left of center is three quarters around.
        assert_eq!(ring_cell(1, 7, 14, 0.5), Some(false));
        assert_eq!(ring_cell(1, 7, 14, 0.8), Some(true));
    }
}
