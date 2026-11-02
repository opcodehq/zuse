//! New-chat defaults use the same model menu as the composer.
use super::{SavePolicy, widgets};
use crate::localization::message as tr;
use crate::{pickers::Pickers, state::AppState, theme::Theme};
use gpui::{prelude::*, *};
use zuse_i18n::messages as msg;
use zuse_protocol::presentation::PermissionMode;

pub struct DefaultsPage {
    picker: Entity<Pickers>,
    permission: widgets::SelectState,
    scroll: widgets::PageScroll,
}
impl DefaultsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let picker = cx.new(|cx| Pickers::new_for_defaults(state, cx));
        cx.observe(&picker, |_, _, cx| cx.notify()).detach();
        cx.observe_global::<super::SettingsStore>(|_: &mut Self, cx| cx.notify())
            .detach();
        Self {
            picker,
            permission: Default::default(),
            scroll: Default::default(),
        }
    }
    pub fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self
            .picker
            .update(cx, |picker, cx| picker.dismiss_settings_menu(true, cx))
        {
            return true;
        }
        if self.permission.is_open() {
            widgets::close_select(self, |this| &mut this.permission, cx);
            return true;
        }
        false
    }
}
impl Render for DefaultsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let preferences = super::current(cx);
        let selected = PermissionMode::ALL
            .iter()
            .position(|mode| *mode == preferences.default_permission_mode)
            .unwrap_or(0);
        let permission = widgets::select(
            "default-permission",
            "Default permission mode",
            &theme,
            |this: &mut Self| &mut this.permission,
        )
        .options(
            PermissionMode::ALL
                .into_iter()
                .map(|mode| widgets::SelectOption::new(mode.label())),
            selected,
        )
        .width(256_f32.min(f32::from(window.viewport_size().width) * 0.4))
        .on_select(|this, index, _, cx| {
            if let Some(mode) = PermissionMode::ALL.get(index) {
                this.picker.update(cx, |picker, cx| {
                    picker.dismiss_settings_menu(false, cx);
                });
                super::update(SavePolicy::Immediate, cx, |settings| {
                    settings.default_permission_mode = *mode
                });
                cx.refresh_windows();
                cx.notify();
            }
        })
        .render(&self.permission, cx);
        let description = preferences
            .default_model
            .as_ref()
            .map(|selection| {
                format!(
                    "Model for new chats · {}",
                    super::accounts::provider_name(selection.provider)
                )
            })
            .unwrap_or_else(|| "Model for new chats.".into());
        let rows = widgets::section_card(&theme)
            .mt_0()
            .child(
                widgets::card_row(&theme, true)
                    .items_start()
                    .child(details(&theme, &tr(cx, msg::settings::settings_page_default_model), &description))
                    .child(self.picker.clone()),
            )
            .child(
                widgets::card_row(&theme, false)
                    .items_start()
                    .child(details(
                        &theme,
                        &tr(cx, msg::settings::settings_page_default_permission_mode),
                        &tr(cx, msg::settings::settings_page_how_new_chats_handle_tool_calls_each_chat_can_override_this_from_the_c),
                    ))
                    .child(permission),
            );
        let content = widgets::settings_page_column(window)
            .child(widgets::section_title(
                &theme,
                &tr(cx, msg::settings::settings_page_default_models),
                &tr(cx, msg::native::choose_how_new_chats_start),
            ))
            .child(widgets::section_stack().child(
                widgets::group(
                    &theme,
                    tr(cx, msg::settings::settings_page_chat_defaults),
                    tr(cx, msg::settings::settings_page_these_choices_apply_when_you_start_a_new_chat_you_can_still_change_eit),
                )
                .child(rows),
            ));
        let area = div()
            .id("default-models-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll.scroll)
            .child(content);
        widgets::scroll_faded("defaults-settings-scroll", area)
    }
}
fn details(theme: &Theme, title: &str, description: &str) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(widgets::row_title(
            theme,
            SharedString::from(title.to_owned()),
        ))
        .child(
            div()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(theme.text_muted)
                .child(SharedString::from(description.to_owned())),
        )
}
