//! Settled threads stay available in the sidebar without archiving the chat
//! or changing its checkout, computer, tabs, pins, or section membership.
use super::*;
use serde_json::Value;
use zuse_protocol::{ChangeRequestState, Chat};

/// About five Settled rows show before the section scrolls on its own.
const SETTLED_ROWS_MAX_HEIGHT: f32 = 5.5 * (SIDEBAR_SETTLED_ROW_HEIGHT + SIDEBAR_LIST_GAP);

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
        state: &AppState,
        chat: &Chat,
        choices: Option<&std::collections::HashMap<String, bool>>,
    ) -> bool {
        choices
            .and_then(|choices| choices.get(&chat.id))
            .copied()
            .unwrap_or_else(|| {
                state
                    .change_request_for_chat(chat)
                    .is_some_and(|pr| pr.state == ChangeRequestState::Merged)
            })
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
        // Only override automatic settlement when reopening a thread whose
        // PR has already merged. Reopening another thread clears its manual
        // choice so a future merge can still move it into Settled.
        let reopen_merged = !settled && !cloud && {
            let state = self.state.read(cx);
            state
                .chats
                .iter()
                .find(|chat| chat.id == id)
                .is_some_and(|chat| Self::local_thread_is_settled(state, chat, None))
        };
        self.pull_settings(cx);
        if settled || reopen_merged {
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
        Some(
            div()
                .id("sidebar-settled-section")
                .mt_auto()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_LIST_GAP))
                .pb(px(Theme::SPACE_SM))
                .child(header)
                .when(!rows.is_empty(), |el| {
                    el.child(
                        div()
                            .id("sidebar-settled-rows")
                            .max_h(px(SETTLED_ROWS_MAX_HEIGHT))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(SIDEBAR_LIST_GAP))
                            .children(rows),
                    )
                })
                .into_any_element(),
        )
    }
}
