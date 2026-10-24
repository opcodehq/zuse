//! The shell's reserved eight-pixel track follows the dashboard scroll handle.
use super::*;
use crate::{palette::rgb, shell::TEXT};
impl UsageDashboard {
    pub(super) fn scroll_view(
        &mut self,
        body: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let handle = self.scroll.clone();
        let rail = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let height = f32::from(bounds.size.height);
                let maximum = f32::from(handle.max_offset().y).max(0.);
                if maximum <= 0. {
                    return;
                }
                let thumb = (height * height / (height + maximum)).max(24.).min(height);
                let position =
                    (-f32::from(handle.offset().y) / maximum).clamp(0., 1.) * (height - thumb);
                window.paint_quad(quad(
                    Bounds::new(
                        bounds.origin + point(px(1.), px(position + 1.)),
                        size(px(6.), px((thumb - 2.).max(1.))),
                    ),
                    px(4.),
                    rgb(TEXT).opacity(0.2),
                    px(0.),
                    transparent_black(),
                    BorderStyle::Solid,
                ));
            },
        )
        .size_full();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if let Some(grab) = this.scroll_drag {
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.scroll_drag = None;
                        return;
                    }
                    let bounds = this.scroll.bounds();
                    let height = f32::from(bounds.size.height);
                    let maximum = f32::from(this.scroll.max_offset().y).max(0.);
                    let thumb = (height * height / (height + maximum)).max(24.).min(height);
                    let fraction = ((f32::from(event.position.y - bounds.top()) - grab)
                        / (height - thumb).max(1.))
                    .clamp(0., 1.);
                    this.scroll
                        .set_offset(point(this.scroll.offset().x, px(-fraction * maximum)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.scroll_drag = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.scroll_drag = None;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("usage-dashboard-scroll")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
            .child(
                div()
                    .id("usage-dashboard-scrollbar")
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(8.))
                    .cursor_default()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let bounds = this.scroll.bounds();
                            let height = f32::from(bounds.size.height);
                            let maximum = f32::from(this.scroll.max_offset().y).max(0.);
                            if maximum <= 0. {
                                return;
                            }
                            let thumb = (height * height / (height + maximum)).max(24.).min(height);
                            let top = (-f32::from(this.scroll.offset().y) / maximum).clamp(0., 1.)
                                * (height - thumb);
                            let position = f32::from(event.position.y - bounds.top());
                            let grab = if position >= top && position <= top + thumb {
                                position - top
                            } else {
                                thumb / 2.
                            };
                            this.scroll_drag = Some(grab);
                            let fraction =
                                ((position - grab) / (height - thumb).max(1.)).clamp(0., 1.);
                            this.scroll
                                .set_offset(point(this.scroll.offset().x, px(-fraction * maximum)));
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .child(rail),
            )
            .into_any_element()
    }
}
