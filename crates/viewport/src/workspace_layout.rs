//! Sidebar sizing and layout events for the browser host.
use super::*;
use crate::shell::{BORDER, column};
use crate::workspace_style as layout;

impl WorkspaceView {
    pub(super) fn workspace_main(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.settings {
            return column()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(self.settings_view(window, cx))
                .into_any_element();
        }
        let chrome = self.selected.is_some() || self.open_file.is_some() || !self.sidebar_open;
        layout::column()
            .flex_1()
            .pr(px(layout::GUTTER))
            .when(!self.sidebar_open, |view| view.pl(px(layout::GUTTER)))
            .child(
                layout::surface()
                    .when(
                        chrome && !self.plugins_browse && self.project_archive.project.is_none(),
                        |view| {
                            view.child(self.header(cx))
                                .when(self.selected.is_some(), |view| {
                                    view.child(self.conversation_tabs(cx))
                                })
                        },
                    )
                    .child(if self.plugins_browse {
                        self.plugins_page_view(cx)
                    } else if self.project_archive.project.is_some() {
                        self.project_archives_view(window, cx)
                    } else if self.project_picker {
                        self.project_picker_view(cx)
                    } else {
                        column()
                            .flex_1()
                            .min_h_0()
                            .child(self.transcript(cx))
                            .child(self.composer(cx))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    pub(super) fn sidebar_width(&self) -> f32 {
        self.preferences
            .sidebar_width
            .map(f32::from)
            .unwrap_or(layout::SIDEBAR_DEFAULT)
    }

    pub(super) fn resize_sidebar(&mut self, width: f32, cx: &mut Context<Self>) {
        self.preferences.sidebar_width = Some(
            width
                .clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX)
                .round() as u16,
        );
        cx.notify();
    }

    pub(super) fn end_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_resize.take().is_some() {
            self.save_preferences();
            cx.notify();
        }
    }

    pub(super) fn cancel_sidebar_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(original) = self.sidebar_resize.take() else {
            return false;
        };
        self.preferences.sidebar_width = original;
        cx.notify();
        true
    }

    pub(super) fn sidebar_separator(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w(px(layout::GUTTER))
            .h_full()
            .flex_none()
            .relative()
            .child(
                div()
                    .id("sidebar-resize")
                    .aria_label("Resize sidebar")
                    .role(Role::Slider)
                    .aria_value(format!("{} pixels", self.sidebar_width()))
                    .aria_numeric_value(self.sidebar_width() as f64)
                    .aria_min_numeric_value(layout::SIDEBAR_MIN as f64)
                    .aria_max_numeric_value(layout::SIDEBAR_MAX as f64)
                    .aria_numeric_value_step(10.)
                    .track_focus(&self.sidebar_resize_focus)
                    .tab_index(0)
                    .absolute()
                    .h_full()
                    .w(px(12.))
                    .left(px(-4.))
                    .cursor_col_resize()
                    .child(
                        div()
                            .absolute()
                            .top(px(14.))
                            .bottom(px(14.))
                            .left(px(5.))
                            .w(px(2.))
                            .rounded_full()
                            .when(self.sidebar_resize.is_some(), |view| view.bg(rgb(BORDER))),
                    )
                    .hover(|view| view.bg(rgb(BORDER).opacity(0.35)))
                    .focus_visible(|view| view.bg(rgb(BORDER)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.sidebar_resize_focus, cx);
                            if event.click_count == 2 {
                                this.preferences.sidebar_width = None;
                                this.save_preferences();
                                this.sidebar_resize = None;
                                cx.notify();
                            } else {
                                this.sidebar_resize = Some(this.preferences.sidebar_width);
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        let width = match event.keystroke.key.as_str() {
                            "left" => this.sidebar_width() - 10.,
                            "right" => this.sidebar_width() + 10.,
                            "home" => layout::SIDEBAR_MIN,
                            "end" => layout::SIDEBAR_MAX,
                            "enter" => layout::SIDEBAR_DEFAULT,
                            _ => return,
                        };
                        this.resize_sidebar(width, cx);
                        this.save_preferences();
                        cx.stop_propagation();
                    })),
            )
            .into_any_element()
    }
}
