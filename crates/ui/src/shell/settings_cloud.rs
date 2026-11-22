//! Settings → Cloud computer and Plan & usage. One Cloud page entity backs
//! every Cloud section, Usage (Cloud compute) included. Local model usage
//! lives in the sidebar footer's usage menu and the Usage route.
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
