//! Cloud computer overview: one row per part of setup, each linking to the
//! page that owns it. Status always comes from loaded account state.
use super::*;

impl RepositoryPanel {
    fn overview_row(
        &self,
        first: bool,
        id: &'static str,
        title: &'static str,
        detail: String,
        status: Div,
        page: CloudPage,
        cx: &mut Context<Self>,
    ) -> Div {
        cs::setting_row(
            first,
            title,
            detail,
            Some(
                row()
                    .gap(px(8.))
                    .items_center()
                    .child(status)
                    .child(
                        cs::button(id, "Open", false, false, false)
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(OpenPage(page)))),
                    )
                    .into_any_element(),
            ),
        )
    }

    pub(super) fn render_overview(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let github = if self.github_connected() {
            let login = self
                .github
                .as_ref()
                .and_then(|g| g.user.as_ref())
                .map(|u| format!("Connected as {}", u.login))
                .unwrap_or_else(|| "Connected".into());
            let installs = self
                .github
                .as_ref()
                .map(|g| g.installations.iter().filter(|i| !i.suspended).count())
                .unwrap_or(0);
            (
                format!(
                    "{login} · {installs} installation{}",
                    if installs == 1 { "" } else { "s" }
                ),
                cs::badge_with_icon("Connected", "check", window),
            )
        } else {
            (
                "Install the Zuse app on the GitHub accounts agents should work on.".into(),
                cs::badge("Needed", true, false),
            )
        };
        let repositories = match self.projects.len() {
            0 => (
                "Choose the repositories cloud computers can check out.".to_owned(),
                cs::badge("Needed", true, false),
            ),
            n => (
                self.projects
                    .iter()
                    .take(3)
                    .map(|p| p.display_name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
                    + &if n > 3 {
                        format!(" and {} more", n - 3)
                    } else {
                        String::new()
                    },
                cs::badge(format!("{n}"), false, true),
            ),
        };
        let agents = self.agents.connected_summary();
        let agents = if agents.is_empty() {
            (
                "Sign in with Codex, Claude or Grok so cloud tasks run on your own plan."
                    .to_owned(),
                cs::badge("Needed", true, false),
            )
        } else {
            (
                agents.join(", "),
                cs::badge_with_icon("Connected", "check", window),
            )
        };
        let machine = match self.machine.image_state() {
            Some(state) => {
                let (label, detail, warning) = machine::image_copy(state);
                (
                    detail.to_owned(),
                    cs::badge(label, warning, !warning && state == "none"),
                )
            }
            None => (
                "Tools, packages and instructions every repository gets.".to_owned(),
                cs::badge("Optional", false, true),
            ),
        };
        let complete = self.guide_step_done(guide::GuideStep::Github)
            && self.guide_step_done(guide::GuideStep::Repositories)
            && self.guide_step_done(guide::GuideStep::Agent);
        let rows = column()
            .child(self.overview_row(
                true,
                "overview-github",
                "GitHub",
                github.0,
                github.1,
                CloudPage::Repositories,
                cx,
            ))
            .child(self.overview_row(
                false,
                "overview-repositories",
                "Repositories",
                repositories.0,
                repositories.1,
                CloudPage::Repositories,
                cx,
            ))
            .child(self.overview_row(
                false,
                "overview-agents",
                "Agent accounts",
                agents.0,
                agents.1,
                CloudPage::Agents,
                cx,
            ))
            .child(self.overview_row(
                false,
                "overview-machine",
                "Machine",
                machine.0,
                machine.1,
                CloudPage::Machine,
                cx,
            ))
            .child(self.overview_row(
                false,
                "overview-environments",
                "Environments",
                "Per-repository setup commands, services and secrets.".into(),
                cs::badge("Optional", false, true),
                CloudPage::Environments,
                cx,
            ));
        let status = cs::group(
            if complete {
                "Ready for cloud tasks"
            } else {
                "Setup isn't finished"
            },
            None,
            Some(
                cs::button(
                    "cloud-guide-open",
                    if complete {
                        "Review setup"
                    } else {
                        "Continue setup"
                    },
                    !complete,
                    !self.can_manage(),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.guide_reopen(cx)))
                .into_any_element(),
            ),
            rows,
        );
        let step = |n: &'static str, title: &'static str, detail: &'static str| {
            row()
                .items_start()
                .gap(px(10.))
                .px(px(12.))
                .py(px(9.))
                .child(
                    div()
                        .flex_none()
                        .size(px(20.))
                        .rounded_full()
                        .border_1()
                        .border_color(palette::rgb(BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.))
                        .text_color(palette::rgb(MUTED))
                        .child(n),
                )
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .child(s::title_text(title))
                        .child(div().mt(px(2.)).child(s::description_text(detail))),
                )
        };
        let flow = cs::group(
            "How a cloud task starts",
            None,
            None,
            column()
                .child(step(
                    "1",
                    "A fresh computer boots from your machine",
                    "Every task gets its own computer. Your machine's tools and instructions are already there.",
                ))
                .child(step(
                    "2",
                    "Only the task's repository is checked out",
                    "A shallow fetch of the branch you picked, with the repository's environment applied.",
                ))
                .child(step(
                    "3",
                    "Your agent starts on your own account",
                    "The agent keeps working after you close Zuse. Follow along or reply from any device.",
                )),
        );
        let mut root = column().gap(px(16.)).child(status).child(flow);
        if let Some(error) = &self.error {
            root = root.child(cs::note(error, true));
        }
        root
    }
}
