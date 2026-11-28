//! Settings → Cloud computer and Plan & usage. The Cloud page entity also
//! backs the Cloud view in Usage, alongside the shared token dashboard.
use super::*;
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
                Navigate::Cloud(view) => {
                    if *view == CloudView::Compute {
                        this.usage_settings_tokens = false;
                    }
                    this.open_settings(SettingsSection::from_cloud_view(*view), cx)
                }
                Navigate::StartCloudChat(start) => this.start_first_cloud_chat(start, cx),
                Navigate::StartSetupChat(start) => this.start_machine_setup_chat(start, cx),
            },
        ));
        self.cloud_billing_page = Some(page.clone());
        page
    }

    fn usage_view_switch(
        &self,
        title: SharedString,
        tokens: SharedString,
        cloud: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut tabs = div()
            .id("usage-view-switch")
            .role(gpui::Role::TabList)
            .aria_label(title)
            .flex()
            .items_center()
            .gap(px(4.));
        for (is_tokens, label, glyph, selector) in [
            (
                false,
                SharedString::from("Cloud"),
                icons::CLOUD,
                "usage-view-cloud",
            ),
            (true, tokens, icons::TAG, "usage-view-tokens"),
        ] {
            let active = is_tokens != cloud;
            let color = if active {
                super::tabs::active_tab_text_color(theme)
            } else {
                theme.text_muted
            };
            let tab = crate::surface_chrome::workspace_tab(selector, active, theme)
                .debug_selector(move || selector.into())
                .role(gpui::Role::Tab)
                .aria_label(label.clone())
                .aria_selected(active)
                .tab_index(0)
                .focus_visible(|style| style.border_1().border_color(theme.accent))
                .cursor_pointer()
                .h(px(28.))
                .px(px(9.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(crate::typography::text_md())
                .text_color(color)
                .when(active, |tab| tab.font_weight(gpui::FontWeight::MEDIUM))
                .child(icon(glyph).size(px(14.)).text_color(color))
                .child(label)
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.usage_settings_tokens = is_tokens;
                        cx.notify();
                        cx.stop_propagation();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.usage_settings_tokens = is_tokens;
                    cx.notify();
                }));
            tabs = tabs.child(tab);
        }
        tabs.into_any_element()
    }

    pub(super) fn usage_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_settings_surface();
        zuse_viewport::palette::set_light(theme.appearance.is_light());
        let subscribed = self.state.read(cx).cloud_access().subscribed();
        let cloud = subscribed && !self.usage_settings_tokens;
        let title = tr(cx, msg::usage::usage_dashboard_usage);
        let tokens = tr(cx, msg::usage::usage_dashboard_tokens);
        let toggle = subscribed.then(|| {
            self.usage_view_switch(
                SharedString::from(title.as_str()),
                SharedString::from(tokens),
                cloud,
                &theme,
                cx,
            )
        });
        let description = if cloud {
            "Machine minutes your cloud computers used this cycle."
        } else {
            "Model tokens, estimated costs, and provider plan limits."
        };
        let header = settings::widgets::settings_page_column(window)
            .flex_none()
            .pb_0()
            .child(
                settings::widgets::section_title_with_action(
                    &theme,
                    &title,
                    None,
                    description,
                    toggle,
                )
                .items_start(),
            );
        let content = if cloud {
            self.hide_usage_dashboard(cx);
            let page = self.ensure_cloud_page(window, cx);
            page.update(cx, |page, cx| page.set_view(CloudView::Compute, cx));
            page.into_any_element()
        } else {
            settings::widgets::settings_page_column(window)
                .h_full()
                .pt_0()
                .pb_0()
                .child(self.usage_view(window, cx))
                .into_any_element()
        };
        div()
            .id("usage-settings")
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
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

    /// "Configure with an agent": a new cloud chat on `start`'s repository
    /// marked as a setup chat, so its computer and agent configure the
    /// account's machine image (see `zuse_protocol::machine_setup`).
    pub(super) fn start_machine_setup_chat(
        &mut self,
        start: &zuse_viewport::cloud_repositories::StartSetupChat,
        cx: &mut Context<Self>,
    ) {
        let Some(mut project) = first_chat_project(&self.state.read(cx), Some(&start.repository))
        else {
            return;
        };
        project[SETUP_PROJECT_FLAG] = serde_json::json!(true);
        self.cloud_first_chat = None;
        self.start_cloud_canvas(project, None, cx);
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
        if project[SETUP_PROJECT_FLAG] == true {
            return Some(self.machine_setup_guide(theme, cx));
        }
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
        if let Route::Settings(section) = self.route
            && !section.reachable(access)
        {
            self.open_settings(section, cx);
        }
    }
}

/// Marks the new-chat project of a setup chat; the composer sends it as
/// the chat's purpose.
pub(crate) const SETUP_PROJECT_FLAG: &str = "machineSetup";

/// One-click starts for a setup chat.
const SETUP_CHAT_PROMPTS: [&str; 3] = [
    "Install Rust and cargo",
    "Add bun and pnpm",
    "Add a repository to my image",
];

impl Shell {
    /// Above a setup chat's composer: what this chat is for.
    fn machine_setup_guide(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let line = |text: &'static str| {
            div()
                .max_w(px(460.))
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
        for (i, prompt) in SETUP_CHAT_PROMPTS.iter().enumerate() {
            prompts = prompts.child(
                div()
                    .id(("machine-setup-prompt", i))
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
        div()
            .id("machine-setup-guide")
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .child(line(
                "This chat configures your machine image. The agent tests a setup script here and proposes the change; you apply it and the image rebuilds.",
            ))
            .child(prompts)
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
