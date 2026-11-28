//! Settled threads stay available in the sidebar without archiving the chat
//! or changing its checkout, computer, tabs, pins, or section membership.
use super::*;
use serde_json::Value;
use zuse_protocol::Chat;

/// Rows an open Settled section shows when the lists above fill the sidebar.
const SETTLED_ROWS_AT_REST: f32 = 5.5;

impl Shell {
    pub(super) fn sidebar_settlement_profile(&self, cloud: bool, cx: &App) -> Option<String> {
        if !cloud {
            return self.active_sidebar_pin_profile_key(cx);
        }
        let state = self.state.read(cx);
        let user = state.cloud_user_id()?;
        let scope = state
            .cloud_organization_id()
            .map_or_else(|| "personal".to_owned(), |org| format!("org:{org}"));
        Some(format!("cloud:{scope}:{user}"))
    }

    pub(super) fn sidebar_settlement_choices(
        &self,
        profile: Option<&str>,
    ) -> Option<&std::collections::HashMap<String, bool>> {
        self.settings.sidebar_settlement_by_profile.get(profile?)
    }

    pub(super) fn local_thread_is_settled(
        chat: &Chat,
        choices: Option<&std::collections::HashMap<String, bool>>,
    ) -> bool {
        choices.and_then(|choices| choices.get(&chat.id)) == Some(&true)
    }

    pub(super) fn set_sidebar_thread_settled(
        &mut self,
        profile: String,
        id: String,
        settled: bool,
        cx: &mut Context<Self>,
    ) {
        let cloud = id.starts_with(CLOUD_ROW_PREFIX);
        if self.sidebar_settlement_profile(cloud, cx).as_deref() != Some(profile.as_str()) {
            return;
        }
        self.pull_settings(cx);
        if settled {
            self.settings
                .sidebar_settlement_by_profile
                .entry(profile)
                .or_default()
                .insert(id, settled);
        } else if let Some(choices) = self
            .settings
            .sidebar_settlement_by_profile
            .get_mut(&profile)
        {
            choices.remove(&id);
            if choices.is_empty() {
                self.settings.sidebar_settlement_by_profile.remove(&profile);
            }
        }
        self.chat_status_hover = None;
        self.chat_hover_resync = true;
        self.schedule_save(cx);
        cx.notify();
    }

    /// Like local threads ([`Self::local_thread_is_settled`]), cloud threads
    /// settle only by an explicit choice.
    pub(super) fn take_settled_cloud_rows(
        &self,
        groups: &mut [super::cloud_sidebar::CloudSidebarGroup],
        cx: &App,
    ) -> Vec<Value> {
        let profile = self.sidebar_settlement_profile(true, cx);
        let Some(choices) = self.sidebar_settlement_choices(profile.as_deref()) else {
            return Vec::new();
        };
        let mut settled = Vec::new();
        for group in groups {
            settled.extend(group.rows.extract_if(.., |row| {
                row["pendingLaunch"].is_null()
                    && row["workspaceId"].as_str().is_some_and(|workspace| {
                        choices.get(&format!("{CLOUD_ROW_PREFIX}{workspace}")) == Some(&true)
                    })
            }));
        }
        settled
    }

    pub(super) fn render_settled_section(
        &self,
        count: usize,
        rows: Vec<AnyElement>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if count == 0 {
            return None;
        }
        let open = self.settled_open;
        let header = div()
            .id("sidebar-settled-toggle")
            .debug_selector(|| "sidebar-settled-toggle".into())
            .role(gpui::Role::Button)
            .aria_label(format!(
                "{} {count} settled threads",
                if open { "Hide" } else { "Show" }
            ))
            .h(px(spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT))
            .flex()
            .items_center()
            .gap(px(6.0))
            // The footer's Cloud glyph below starts a few points in; the
            // label lines up with it, and the chevron with the gear.
            .pl(px(SIDEBAR_CARD_PAD_X + 3.0))
            .pr(px(SIDEBAR_CARD_PAD_X + 1.0))
            .rounded(px(SIDEBAR_ROW_RADIUS))
            .text_color(theme.text_muted)
            .cursor_pointer()
            // Reads like the Threads heading above the list.
            .child(
                div()
                    .flex_1()
                    .text_size(crate::typography::text_sm())
                    .line_height(crate::typography::ui_rems(16.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Settled"),
            )
            .child(
                div()
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted.opacity(0.8))
                    .child(count.to_string()),
            )
            .child(
                icon(if open {
                    icons::ALT_ARROW_DOWN
                } else {
                    icons::ALT_ARROW_RIGHT
                })
                .size(px(14.0))
                .text_color(theme.text_muted),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.settled_open = !this.settled_open;
                cx.notify();
            }));
        // Open, the section asks for about five rows and grows into any
        // space the lists above leave empty; its rows scroll on their own.
        let row_count = rows.len();
        let grows = open && row_count > 0;
        let basis = spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT
            + Theme::SPACE_SM
            + (row_count as f32).min(SETTLED_ROWS_AT_REST)
                * (SIDEBAR_SETTLED_ROW_HEIGHT + SIDEBAR_LIST_GAP);
        Some(
            div()
                .id("sidebar-settled-section")
                .debug_selector(|| "sidebar-settled-section".into())
                .px(px(Theme::SPACE_SM))
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_LIST_GAP))
                .pb(px(Theme::SPACE_SM))
                .when(grows, |el| {
                    el.flex_grow(1.0)
                        .flex_shrink_0()
                        .flex_basis(px(basis))
                        .min_h_0()
                })
                .when(!grows, |el| el.mt_auto().flex_none())
                .child(header)
                .when(grows, |el| {
                    el.child(
                        div()
                            .id("sidebar-settled-rows")
                            .debug_selector(|| "sidebar-settled-rows".into())
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(SIDEBAR_LIST_GAP))
                            // Rows keep their height inside the scroll area.
                            .children(rows.into_iter().map(|row| div().flex_none().child(row))),
                    )
                })
                .into_any_element(),
        )
    }
}
