//! Settings → Cloud computer and Plan & usage. One Cloud page entity backs
//! every Cloud section; Usage pairs model usage with Cloud compute.
use super::*;
use gpui::{Div, Stateful};
use settings::cloud_billing::{CloudBillingPage, CloudView, Navigate};

impl Shell {
    pub(super) fn ensure_cloud_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CloudBillingPage> {
        if let Some(page) = &self.cloud_billing_page {
            return page.clone();
        }
        let state = self.state.clone();
        let page = cx.new(|cx| CloudBillingPage::new(state, window, cx));
        self.cloud_page_sub = Some(cx.subscribe(
            &page,
            |this, _, event: &Navigate, cx| match event {
                Navigate::Cloud(CloudView::Compute) => {
                    this.set_usage_settings_compute(true, cx);
                    this.open_settings(SettingsSection::Usage, cx);
                }
                Navigate::Cloud(view) => {
                    this.open_settings(SettingsSection::from_cloud_view(*view), cx)
                }
                Navigate::StartCloudChat(start) => this.start_first_cloud_chat(start, cx),
            },
        ));
        self.cloud_billing_page = Some(page.clone());
        page
    }

    /// The end of onboarding: a new chat on Cloud with the repository picked
    /// in the last step, the composer focused and a short guide above it.
    /// Without a pick, the first cloud repository that can start one; with
    /// none at all, the plain new-chat canvas.
    pub(super) fn start_first_cloud_chat(
        &mut self,
        start: &zuse_viewport::cloud_repositories::StartChat,
        cx: &mut Context<Self>,
    ) {
        let project = first_chat_project(&self.state.read(cx), start.repository.as_ref());
        match project {
            Some(project) => {
                self.cloud_first_chat = Some(FirstCloudChat {
                    project_id: project["projectId"].as_str().unwrap_or_default().to_owned(),
                    image: start.image,
                });
                // The same repository's checkout here, if any, stays the
                // Local alternative.
                let local_space = {
                    let state = self.state.read(cx);
                    let identity = project["repositoryIdentity"].as_str().unwrap_or_default();
                    state
                        .spaces
                        .iter()
                        .find(|space| {
                            space.remote_repository.as_deref().is_some_and(|remote| {
                                crate::cloud_destination::normalize(remote)
                                    == crate::cloud_destination::normalize(identity)
                            })
                        })
                        .map(|space| space.id.clone())
                };
                self.start_cloud_canvas(project, local_space, cx);
            }
            None => self.open_new_session(None, cx),
        }
    }

    /// The empty first cloud chat's guide: what happens when you send, a
    /// line about the image while it builds, and prompts to start from.
    /// Shown for the chat onboarding opened, and for any new chat on Cloud
    /// before the account has run one.
    pub(super) fn first_cloud_chat_guide(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.state.read(cx);
        let project = state.cloud_project.as_ref()?;
        let opened = self
            .cloud_first_chat
            .as_ref()
            .filter(|guide| project["projectId"] == guide.project_id.as_str());
        if opened.is_none() && !state.cloud_sidebar_workspaces().is_empty() {
            return None;
        }
        let building = opened.is_some_and(|guide| {
            guide.image == zuse_viewport::cloud_repositories::ImageReadiness::Building
        });
        let line = |text: &'static str| {
            div()
                .max_w(px(440.))
                .text_center()
                .text_size(crate::typography::text_sm())
                .text_color(theme.text_muted)
                .child(text)
        };
        let mut prompts = div()
            .mt(px(6.))
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(px(6.));
        for (i, prompt) in FIRST_CHAT_PROMPTS.iter().enumerate() {
            prompts = prompts.child(
                div()
                    .id(("first-cloud-chat-prompt", i))
                    .role(gpui::Role::Button)
                    .aria_label(*prompt)
                    .tab_index(0)
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text)
                    .hover(|v| v.bg(theme.element_hover))
                    .child(*prompt)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.composer
                            .update(cx, |composer, cx| composer.suggest_prompt(prompt, cx));
                    })),
            );
        }
        Some(
            div()
                .id("first-cloud-chat-guide")
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.))
                .child(line(
                    "A cloud computer starts from your image with this repository, and your agent signs in with your connected account.",
                ))
                .when(building, |v| {
                    v.child(line(
                        "Your image is still building, so this chat starts without it and takes a little longer.",
                    ))
                })
                .child(prompts)
                .into_any_element(),
        )
    }

    /// Opens first-run Cloud onboarding on Cloud computer → Overview,
    /// optionally at a step (`welcome`, `repositories`, `setup`, `agents`,
    /// `start`). The "Set up Cloud" button and review fixtures use it.
    pub fn open_cloud_onboarding(
        &mut self,
        step: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = self.ensure_cloud_page(window, cx);
        page.update(cx, |page, cx| page.open_onboarding(step, cx));
        self.open_settings(SettingsSection::Cloud, cx);
    }

    /// Review fixtures open Usage on its Cloud compute tab.
    #[cfg(feature = "cloud-computer-fixture")]
    pub fn fixture_usage_compute(&mut self, cx: &mut Context<Self>) {
        self.set_usage_settings_compute(true, cx);
    }

    /// Subscribing, or a subscription ending, applies at once: the Settings
    /// menu re-renders from [`AppState::cloud_access`], and a page that is no
    /// longer offered gives way to Billing (Cloud pages) or General. Waits
    /// for a settled answer so start-up and engine restarts never bounce the
    /// user off a Cloud page.
    pub(super) fn follow_cloud_access(&mut self, cx: &mut Context<Self>) {
        let access = self.state.read(cx).cloud_access();
        if !access.settled() {
            return;
        }
        // Subscribing while Billing is open (the checkout tab returning)
        // continues straight into first-run setup on Cloud computer.
        let subscribed_now = access.subscribed() && self.cloud_seen_unsubscribed;
        self.cloud_seen_unsubscribed = access.signed_in() && !access.subscribed();
        if subscribed_now && self.route == Route::Settings(SettingsSection::Billing) {
            self.open_settings(SettingsSection::Cloud, cx);
            return;
        }
        if self.usage_settings_compute && !access.subscribed() {
            self.set_usage_settings_compute(false, cx);
        }
        if let Route::Settings(section) = self.route
            && !section.reachable(access)
        {
            self.open_settings(section, cx);
        }
    }

    fn set_usage_settings_compute(&mut self, compute: bool, cx: &mut Context<Self>) {
        // Only subscribers have a Cloud compute tab to switch to.
        let compute = compute && !self.state.read(cx).cloud_access().withholds_cloud();
        if self.usage_settings_compute == compute {
            return;
        }
        let here = self.route == Route::Settings(SettingsSection::Usage);
        self.usage_settings_compute = compute;
        if here {
            if compute {
                if let Some(dashboard) = self.usage_dashboard.clone() {
                    dashboard.update(cx, |dashboard, cx| dashboard.hide(cx));
                }
            } else {
                self.usage_pending = true;
            }
        }
        cx.notify();
    }

    fn usage_tab(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        compute: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .role(gpui::Role::Tab)
            .aria_label(label)
            .aria_selected(selected)
            .tab_index(0)
            .h(px(26.))
            .px(px(10.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_size(crate::typography::text_xs())
            .text_color(if selected {
                theme.text
            } else {
                theme.text_muted
            })
            .when(selected, |v| {
                v.bg(theme.settings_frame_bg())
                    .border_1()
                    .border_color(theme.border)
            })
            .hover(|v| v.text_color(theme.text))
            .focus_visible(|v| v.border_2().border_color(theme.accent))
            .on_click(
                cx.listener(move |this, _, _, cx| this.set_usage_settings_compute(compute, cx)),
            )
            .child(label)
    }

    /// Usage: model usage from this computer's agents, or Cloud compute.
    pub(super) fn usage_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_settings_surface();
        // Model usage alone unless the account has a Cloud subscription: no
        // Cloud compute tab for local-only builds, signed-out or unsubscribed
        // accounts.
        let cloud = self.state.read(cx).cloud_access().subscribed();
        let compute = cloud && self.usage_settings_compute;
        let tabs = div()
            .id("usage-settings-tabs")
            .role(gpui::Role::TabList)
            .aria_label("Usage")
            .mt(px(16.))
            .flex()
            .gap(px(4.))
            .child(self.usage_tab("usage-tab-models", "Models", !compute, false, &theme, cx))
            .child(self.usage_tab(
                "usage-tab-compute",
                "Cloud compute",
                compute,
                true,
                &theme,
                cx,
            ));
        let header = settings::widgets::settings_page_column(window)
            .pb(px(12.))
            .child(settings::widgets::section_title(
                &theme,
                "Usage",
                if compute {
                    "What your cloud computers used this cycle, by computer and by chat."
                } else if cloud {
                    "Tokens and cost by model, provider and chat, from the agents you run here and in Cloud."
                } else {
                    "Tokens and cost by model, provider and chat, from the agents you run on this computer."
                },
            ))
            .when(cloud, |header| header.child(tabs));
        if compute {
            let page = self.ensure_cloud_page(window, cx);
            page.update(cx, |page, cx| page.set_view(CloudView::Compute, cx));
            return div()
                .id("usage-settings")
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_none().child(header))
                .child(div().flex_1().min_h_0().child(page))
                .into_any_element();
        }

        let padding = settings::widgets::page_padding(window);
        let body = div()
            .flex_none()
            .w_full()
            .max_w(px(768. + 2. * padding))
            .mx_auto()
            .px(px((padding - 24.).max(0.)))
            .child(self.usage_view(window, cx));
        let rail = settings::widgets::rail(
            &mut self.usage_settings_scroll,
            "usage-settings-scrollbar",
            &theme,
            cx,
            |this: &mut Shell| &mut this.usage_settings_scroll,
        );
        div()
            .id("usage-settings")
            .relative()
            .size_full()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.usage_settings_scroll.set_list_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(
                div()
                    .id("usage-settings-scroll")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.usage_settings_scroll.scroll)
                    .child(div().flex_none().child(header))
                    .child(body),
            )
            .children(rail)
            .into_any_element()
    }
}

/// One-click starts for a first chat on a repository.
const FIRST_CHAT_PROMPTS: [&str; 3] = [
    "Explain this codebase",
    "Find and fix a failing test",
    "Suggest a small first improvement",
];

/// The guide a first cloud chat shows until its first message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FirstCloudChat {
    pub project_id: String,
    pub image: zuse_viewport::cloud_repositories::ImageReadiness,
}

/// The cloud repository a first chat opens on: the listed row for the one
/// picked in onboarding (or, before the list catches up, that repository as
/// the panel saw it), else the first listed repository that can start a chat.
pub(crate) fn first_chat_project(
    state: &AppState,
    picked: Option<&zuse_viewport::cloud_repositories::FirstChatRepository>,
) -> Option<serde_json::Value> {
    let projects = state.cloud_sidebar_projects();
    if let Some(repo) = picked {
        if let Some(row) = projects
            .iter()
            .find(|row| row["projectId"] == repo.project_id.as_str())
        {
            return Some(row.clone());
        }
        let owner = state
            .cloud_organization_id()
            .map(|org| format!("organization:{org}"))
            .or_else(|| state.cloud_user_id().map(str::to_owned))?;
        return Some(serde_json::json!({
            "projectId": repo.project_id,
            "repositoryIdentity": repo.repository_identity,
            "repositoryUrl": repo.repository_url,
            "displayName": repo.display_name,
            "defaultBranch": repo.default_branch,
            "accountId": owner,
            "providerId": "boxd",
        }));
    }
    projects
        .into_iter()
        .find(|project| project["canCreate"] != false)
}
