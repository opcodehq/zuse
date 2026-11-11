//! Workspace surfaces and platform titlebar controls.
use super::*;
use crate::localization::message as tr;
use zuse_i18n::messages as msg;
use zuse_viewport::workspace_style as layout;

impl Shell {
    pub(super) fn workspace_gutter(&self) -> f32 {
        if matches!(
            self.route,
            Route::Chat
                | Route::PullRequests
                | Route::Plugins
                | Route::Archives
                | Route::Usage
                | Route::Zot
        ) {
            layout::GUTTER
        } else {
            0.
        }
    }

    pub(super) fn main_chrome_visible(&self, cx: &App) -> bool {
        self.workspace_gutter() == 0.
            || matches!(
                self.route,
                Route::Chat | Route::PullRequests | Route::Plugins
            )
            || self.has_conversation(cx)
            || self.settings.sidebar_collapsed
    }

    /// The sidebar's surface. It paints no fill of its own: the sidebar is
    /// the window glass (translucent over the desktop where the platform
    /// blurs it), which sets it apart from the solid conversation card.
    pub(super) fn workspace_surface(&self, _cx: &App) -> gpui::Div {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .rounded(px(if self.workspace_gutter() > 0. {
                layout::RADIUS
            } else {
                0.
            }))
            .overflow_hidden()
    }

    pub(super) fn workspace_sidebar_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let width = self.sidebar_now();
        if self.workspace_gutter() == 0. || width < 1. {
            return Empty.into_any_element();
        }
        let leading = if cfg!(target_os = "macos") && !self.fullscreen.unwrap_or(false) {
            80.
        } else {
            12. + caption_buttons_width(self.linux_left_caption_count())
        };
        let theme = Theme::of(cx).clone();
        let navigation = matches!(self.route, Route::PullRequests)
            .then(|| self.render_titlebar_navigation(&theme, cx));
        self.titlebar_drag_region(
            "workspace-sidebar-titlebar",
            div()
                .absolute()
                .left(px(layout::GUTTER))
                .top(px(layout::GUTTER))
                .w(px((width - layout::GUTTER).max(0.)))
                .h(px(36.))
                .overflow_hidden()
                .pl(px(leading))
                .pr(px(4.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .debug_selector(|| "workspace-app-logo".into())
                        .size(px(16.))
                        .flex_none()
                        .child(icon(icons::ZUSE_LOGO).size_full().text_color(theme.text)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(tr(cx, msg::chat::top_bar_zuse_beta)),
                )
                .children(navigation)
                .child(
                    zuse_viewport::sidebar_controls::footer_button(
                        "workspace-hide-sidebar",
                        "Hide projects panel",
                    )
                    .debug_selector(|| "workspace-hide-sidebar".into())
                    .track_focus(&self.workspace_sidebar_focus)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .w(px(28.))
                    .child(
                        icon(icons::SPLIT_COLUMNS)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.toggle_sidebar(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                ),
            cx,
        )
        .into_any_element()
    }
}
