use super::{AppearancePage, selection_tone};
use crate::settings::{
    self, NewThreadBackgroundEffect, NewThreadComposerBackground, WallpaperLook, widgets,
};
use crate::theme::Theme;
use gpui::{AnyElement, Bounds, Context, Pixels, Render, Window, div, prelude::*, px};
use std::{cell::Cell, path::Path, rc::Rc};

#[derive(Clone, Copy)]
enum Control {
    Brightness,
    Blur,
    Opacity,
    Fade,
    Edge,
    Input,
}

impl Control {
    const ALL: [Self; 6] = [
        Self::Brightness,
        Self::Blur,
        Self::Opacity,
        Self::Fade,
        Self::Edge,
        Self::Input,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Brightness => "Brightness",
            Self::Blur => "Blur",
            Self::Opacity => "Image opacity",
            Self::Fade => "Bottom fade",
            Self::Edge => "Composer feather",
            Self::Input => "Input opacity",
        }
    }

    fn kind(self) -> (u8, u8, u8, &'static str) {
        match self {
            Self::Brightness => (50, 150, 5, "%"),
            Self::Blur => (0, 20, 1, " px"),
            Self::Opacity | Self::Fade => (0, 100, 5, "%"),
            Self::Edge => (0, 35, 1, "%"),
            Self::Input => (65, 100, 5, "%"),
        }
    }

    fn value(self, look: &mut WallpaperLook) -> &mut u8 {
        match self {
            Self::Brightness => &mut look.brightness,
            Self::Blur => &mut look.blur,
            Self::Opacity => &mut look.opacity,
            Self::Fade => &mut look.bottom_fade,
            Self::Edge => &mut look.soft_edge,
            Self::Input => &mut look.input_opacity,
        }
    }

    fn at_x(self, x: Pixels, bounds: Bounds<Pixels>) -> u8 {
        let (minimum, maximum, step, _) = self.kind();
        let fraction = (f32::from(x - bounds.left() - px(6.0))
            / f32::from((bounds.size.width - px(12.0)).max(px(1.0))))
        .clamp(0.0, 1.0);
        let steps = f32::from(maximum - minimum) / f32::from(step);
        (minimum + (fraction * steps).round() as u8 * step).min(maximum)
    }
}

#[derive(Clone)]
pub(super) struct WallpaperSliderDrag(Control, Rc<Cell<Option<Bounds<Pixels>>>>);

impl Render for WallpaperSliderDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl AppearancePage {
    fn set_wallpaper_control(&mut self, control: Control, value: u8, cx: &mut Context<Self>) {
        let mut look = settings::current(cx).wallpaper_look;
        *control.value(&mut look) = value;
        self.compare_wallpaper = false;
        settings::set_wallpaper_look(look, cx);
        cx.notify();
    }

    pub(super) fn on_wallpaper_slider_drag(
        &mut self,
        event: &gpui::DragMoveEvent<WallpaperSliderDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let payload = event.drag(cx);
        if let Some(bounds) = payload.1.get() {
            self.set_wallpaper_control(
                payload.0,
                payload.0.at_x(event.event.position.x, bounds),
                cx,
            );
        }
    }

    pub(super) fn render_wallpaper_panel(
        &self,
        background: &NewThreadComposerBackground,
        effect: NewThreadBackgroundEffect,
        look: WallpaperLook,
        theme: &Theme,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let showing_original = self.compare_wallpaper;
        let preview_look = if showing_original {
            WallpaperLook::default()
        } else {
            look
        };
        let preview_effect = if showing_original {
            NewThreadBackgroundEffect::None
        } else {
            effect
        };
        let image = crate::new_thread_background_effects::prepare_look(
            preview_effect,
            theme,
            Path::new(&background.path),
            preview_look,
            cx,
        );
        let composer_bounds = crate::new_thread_background_mask::SurfaceBounds::default();
        let focus = selection_tone(theme);
        let stage = div()
            .relative()
            .w_full()
            .h(px(if wide { 232.0 } else { 178.0 }))
            .rounded(px(9.0))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg)
            .when_some(image, |stage, image| {
                stage.child(div().absolute().inset_0().child(
                    crate::new_thread_background_mask::artwork(
                        image,
                        background.adjustment,
                        preview_look,
                        composer_bounds.clone(),
                        crate::shell::new_thread_background_opacity(theme.is_frost()),
                    ),
                ))
            })
            .child(crate::frost::layered(
                div()
                    .absolute()
                    .left(gpui::relative(0.18))
                    .right(gpui::relative(0.18))
                    .bottom(px(18.0))
                    .h(px(62.0))
                    .rounded(px(9.0))
                    .border_1()
                    .border_color(theme.composer_glass_border())
                    .bg(theme.wallpaper_composer_fill(preview_look.input_opacity))
                    .p(px(8.0))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .child("Ask anything…")
                    .child(
                        div().flex().justify_end().child(
                            crate::popover::dither_key(
                                div()
                                    .size(px(18.0))
                                    .rounded(px(4.0))
                                    .flex()
                                    .items_center()
                                    .justify_center(),
                                theme,
                                crate::popover::DitherKey::Primary,
                                4.0,
                            )
                            .child(
                                crate::icons::icon(crate::icons::DITHER_SEND)
                                    .relative()
                                    .size(px(10.0))
                                    .text_color(gpui::white()),
                            ),
                        ),
                    )
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| composer_bounds.set(Some(bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
            ));
        let quiet = |label, id| {
            widgets::text_action(theme, widgets::ActionTone::Quiet, label)
                .id(id)
                .px(px(8.0))
                .tab_index(0)
                .role(gpui::Role::Button)
                .focus_visible(|style| style.border_1().border_color(focus))
        };
        let mut sliders = div().flex().flex_col().gap(px(9.0));
        for controls in Control::ALL.chunks(if wide { 2 } else { 1 }) {
            sliders = sliders.child(
                div().flex().gap(px(24.0)).children(
                    controls
                        .iter()
                        .map(|control| slider(*control, look, theme, cx)),
                ),
            );
        }
        div()
            .border_t_1()
            .border_color(theme.border)
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(widgets::row_title(theme, "New thread preview"))
                    .child(
                        quiet(
                            if showing_original {
                                "Show adjustments"
                            } else {
                                "Compare original"
                            },
                            "wallpaper-compare",
                        )
                        .aria_selected(showing_original)
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.compare_wallpaper = !page.compare_wallpaper;
                            cx.notify();
                        })),
                    ),
            )
            .child(stage)
            .child(sliders)
            .when(look != WallpaperLook::default(), |panel| {
                panel.child(div().flex().justify_end().child(
                    quiet("Reset adjustments", "wallpaper-reset-look").on_click(cx.listener(
                        |page, _, _, cx| {
                            page.compare_wallpaper = false;
                            settings::set_wallpaper_look(WallpaperLook::default(), cx);
                            cx.notify();
                        },
                    )),
                ))
            })
            .into_any_element()
    }
}

fn slider(
    control: Control,
    mut look: WallpaperLook,
    theme: &Theme,
    cx: &mut Context<AppearancePage>,
) -> gpui::Div {
    let value = *control.value(&mut look);
    let (minimum, maximum, step, unit) = control.kind();
    let fraction = f32::from(value - minimum) / f32::from(maximum - minimum);
    let position = Rc::new(Cell::new(None));
    let measured = position.clone();
    let press = position.clone();
    let line = selection_tone(theme);
    let label = format!("{value}{unit}");
    let thumb = div()
        .absolute()
        .left(gpui::relative(fraction))
        .ml(px(-5.0))
        .top(px(-3.0))
        .size(px(10.0))
        .rounded_full()
        .bg(line);
    let track = div()
        .absolute()
        .left(px(6.0))
        .right(px(6.0))
        .top(px(9.0))
        .h(px(4.0))
        .rounded_full()
        .bg(theme.border_strong)
        .child(
            div()
                .h_full()
                .w(gpui::relative(fraction))
                .rounded_full()
                .bg(line.opacity(0.64)),
        )
        .child(thumb);
    let id = format!("wallpaper-slider-{}", super::slug(control.name()));
    let input = div()
        .id(gpui::SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .relative()
        .w_full()
        .h(px(22.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .tab_index(0)
        .role(gpui::Role::Slider)
        .aria_label(control.name())
        .aria_value(label.clone())
        .aria_numeric_value(f64::from(value))
        .aria_min_numeric_value(f64::from(minimum))
        .aria_max_numeric_value(f64::from(maximum))
        .aria_numeric_value_step(f64::from(step))
        .focus_visible(|style| style.border_1().border_color(line))
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |page, event: &gpui::MouseDownEvent, _, cx| {
                if let Some(bounds) = press.get() {
                    page.set_wallpaper_control(control, control.at_x(event.position.x, bounds), cx);
                }
            }),
        )
        .on_drag(
            WallpaperSliderDrag(control, position),
            |payload, _, _, cx| cx.new(|_| payload.clone()),
        )
        .on_key_down(
            cx.listener(move |page, event: &gpui::KeyDownEvent, window, cx| {
                let mut look = settings::current(cx).wallpaper_look;
                let current = *control.value(&mut look);
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => current.saturating_sub(step).max(minimum),
                    "right" | "up" => current.saturating_add(step).min(maximum),
                    "home" => minimum,
                    "end" => maximum,
                    _ => return,
                };
                cx.stop_propagation();
                window.prevent_default();
                page.set_wallpaper_control(control, next, cx);
            }),
        )
        .child(
            gpui::canvas(
                move |bounds, _, _| measured.set(Some(bounds)),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .child(track);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .justify_between()
                .gap(px(8.0))
                .child(widgets::row_title(theme, control.name()))
                .child(
                    div()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(label),
                ),
        )
        .child(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn clicking_the_wallpaper_brightness_slider_changes_settings(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wallpaper.png");
        image::RgbaImage::from_pixel(32, 24, image::Rgba([120, 80, 50, 255]))
            .save(&path)
            .unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            settings::init(
                settings::UiSettings {
                    new_thread_composer_background: Some(NewThreadComposerBackground {
                        path: path.to_string_lossy().into_owned(),
                        name: "wallpaper.png".into(),
                        adjustment: Default::default(),
                    }),
                    ..Default::default()
                },
                directory.path(),
                cx,
            );
        });
        let (_, cx) = cx.add_window_view(|_, cx| AppearancePage::new(cx));
        cx.run_until_parked();
        let center = cx.update(|window, cx| {
            window.draw(cx).clear();
            let viewport = window.viewport_size();
            gpui::point(viewport.width / 2.0, viewport.height / 2.0)
        });
        let initial = cx.debug_bounds("wallpaper-slider-brightness").unwrap();
        cx.simulate_mouse_move(center, None, Default::default());
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: center,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), center.y - initial.center().y)),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let slider = cx.debug_bounds("wallpaper-slider-brightness").unwrap();
        cx.simulate_click(
            gpui::point(slider.right() - px(3.0), slider.center().y),
            Default::default(),
        );
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(settings::current(cx).wallpaper_look.brightness, 150));
    }
}
