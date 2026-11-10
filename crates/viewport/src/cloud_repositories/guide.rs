//! Guided Cloud setup. Every step embeds the same group Settings renders, so
//! the guide and Settings can never disagree about what is configured.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GuideStep {
    Github,
    Repositories,
    Agent,
    Machine,
    Environment,
}

impl GuideStep {
    pub(super) const ALL: [GuideStep; 5] = [
        GuideStep::Github,
        GuideStep::Repositories,
        GuideStep::Agent,
        GuideStep::Machine,
        GuideStep::Environment,
    ];
    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    fn slug(self) -> &'static str {
        match self {
            GuideStep::Github => "github",
            GuideStep::Repositories => "repositories",
            GuideStep::Agent => "agent",
            GuideStep::Machine => "machine",
            GuideStep::Environment => "environment",
        }
    }
    fn title(self) -> &'static str {
        match self {
            GuideStep::Github => "Connect GitHub",
            GuideStep::Repositories => "Choose repositories",
            GuideStep::Agent => "Connect a coding agent",
            GuideStep::Machine => "Customize your machine",
            GuideStep::Environment => "Prepare a repository",
        }
    }
    fn summary(self) -> &'static str {
        match self {
            GuideStep::Github => {
                "Install the Zuse app on the accounts whose code agents should work on."
            }
            GuideStep::Repositories => "Pick the repositories cloud computers can check out.",
            GuideStep::Agent => "Sign in with Codex, Claude or Grok so tasks run on your own plan.",
            GuideStep::Machine => {
                "Optional: tools, packages and instructions every repository gets, built into your image."
            }
            GuideStep::Environment => "Optional: a repository's own setup commands and services.",
        }
    }
    fn optional(self) -> bool {
        matches!(self, GuideStep::Machine | GuideStep::Environment)
    }
}

/// Session-local guide state. Completion always comes from real account state.
#[derive(Default)]
pub(super) struct GuideState {
    step: Option<GuideStep>,
    dismissed: bool,
    open: bool,
}

fn lime() -> Hsla {
    palette::tone(
        hsla(83. / 360., 0.72, 0.46, 1.),
        hsla(83. / 360., 0.74, 0.43, 1.),
    )
}

impl RepositoryPanel {
    pub(super) fn guide_step_done(&self, step: GuideStep) -> bool {
        match step {
            GuideStep::Github => self.github_connected(),
            GuideStep::Repositories => !self.projects.is_empty(),
            GuideStep::Agent => self.agents.any_connected(),
            GuideStep::Machine => self.machine.customized(),
            GuideStep::Environment => false,
        }
    }
    fn guide_required_done(&self) -> bool {
        GuideStep::ALL
            .iter()
            .filter(|s| !s.optional())
            .all(|s| self.guide_step_done(*s))
    }
    /// The guide opens by itself until the required steps are done, and on
    /// request afterwards. Loading never flashes it.
    pub(super) fn guide_visible(&self) -> bool {
        self.loaded
            && self.owner.is_some()
            && self.can_manage()
            && (self.guide.open || (!self.guide.dismissed && !self.guide_required_done()))
    }
    fn guide_current(&self) -> GuideStep {
        self.guide.step.unwrap_or_else(|| {
            GuideStep::ALL
                .into_iter()
                .find(|s| !s.optional() && !self.guide_step_done(*s))
                .unwrap_or(GuideStep::Environment)
        })
    }
    fn guide_go(&mut self, step: GuideStep, cx: &mut Context<Self>) {
        self.guide.step = Some(step);
        cx.notify();
    }
    fn guide_close(&mut self, cx: &mut Context<Self>) {
        self.guide.open = false;
        self.guide.dismissed = true;
        self.guide.step = None;
        cx.notify();
    }
    pub(super) fn guide_reopen(&mut self, cx: &mut Context<Self>) {
        self.guide.open = true;
        self.guide.step = None;
        cx.notify();
    }

    fn guide_header(&self) -> Div {
        let done = GuideStep::ALL
            .iter()
            .filter(|s| !s.optional() && self.guide_step_done(**s))
            .count();
        let required = GuideStep::ALL.iter().filter(|s| !s.optional()).count();
        let mut bar = row().gap(px(4.)).mt(px(10.));
        for step in GuideStep::ALL.iter().filter(|s| !s.optional()) {
            bar = bar.child(div().flex_1().h(px(3.)).rounded_full().bg(
                if self.guide_step_done(*step) {
                    lime()
                } else {
                    palette::rgb(BORDER).into()
                },
            ));
        }
        div()
            .px(px(10.))
            .pt(px(6.))
            .pb(px(10.))
            .child(
                row()
                    .items_baseline()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(16.))
                            .line_height(px(22.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette::rgb(TEXT))
                            .child("Set up Cloud"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(palette::rgb(MUTED))
                            .child(SharedString::from(format!("{done} of {required} done"))),
                    ),
            )
            .child(
                div()
                    .mt(px(2.))
                    .child(s::description_text(
                        "Three required steps. Tasks then start on a fresh computer with your repository and agent ready.",
                    )),
            )
            .child(bar)
    }

    fn guide_marker(&self, step: GuideStep, current: bool) -> Div {
        let done = self.guide_step_done(step);
        let base = div()
            .flex_none()
            .size(px(20.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.))
            .font_weight(FontWeight::MEDIUM);
        if done {
            base.bg(lime().opacity(0.16))
                .child(icon("check").size(px(11.)).text_color(lime()))
        } else if current {
            base.border_1()
                .border_color(lime())
                .text_color(palette::rgb(TEXT))
                .child(SharedString::from((step.index() + 1).to_string()))
        } else {
            base.border_1()
                .border_color(palette::rgb(BORDER))
                .text_color(palette::rgb(MUTED))
                .child(SharedString::from((step.index() + 1).to_string()))
        }
    }

    fn guide_checklist(&self, current: GuideStep, cx: &mut Context<Self>) -> Div {
        let mut card = s::card();
        for (index, step) in GuideStep::ALL.into_iter().enumerate() {
            let selected = step == current;
            let done = self.guide_step_done(step);
            let status = if done {
                Some(cs::badge("Done", false, false))
            } else if step.optional() {
                Some(cs::badge("Optional", false, true))
            } else {
                None
            };
            let row_view = div()
                .id(SharedString::from(format!(
                    "cloud-guide-step-{}",
                    step.slug()
                )))
                .role(Role::Tab)
                .aria_label(step.title())
                .aria_selected(selected)
                .tab_index(0)
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(9.))
                .cursor_pointer()
                .when(index > 0, |v| {
                    v.border_t_1().border_color(palette::rgb(BORDER))
                })
                .when(selected, |v| v.bg(s::frame_bg().opacity(0.6)))
                .hover(|v| v.bg(s::frame_bg().opacity(0.4)))
                .on_click(cx.listener(move |this, _, _, cx| this.guide_go(step, cx)))
                .child(self.guide_marker(step, selected))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(s::title_text(step.title()))
                        .child(div().mt(px(2.)).child(s::description_text(step.summary()))),
                )
                .when_some(status, |v, badge| v.child(badge));
            card = card.child(row_view);
        }
        card
    }

    fn guide_footer(&self, current: GuideStep, cx: &mut Context<Self>) -> Div {
        let last = current.index() + 1 == GuideStep::ALL.len();
        let ready = self.guide_step_done(current) || current.optional();
        let next_label = if last {
            if self.guide_required_done() {
                "Start using Cloud"
            } else {
                "Finish later"
            }
        } else if current.optional() && !self.guide_step_done(current) {
            "Skip"
        } else {
            "Continue"
        };
        let mut actions = row().gap(px(8.)).items_center();
        if current.index() > 0 {
            let previous = GuideStep::ALL[current.index() - 1];
            actions = actions.child(
                cs::button("cloud-guide-back", "Back", false, false, false)
                    .on_click(cx.listener(move |this, _, _, cx| this.guide_go(previous, cx))),
            );
        }
        let continue_button =
            crate::styling::dither_action("cloud-guide-continue", next_label, ready, true)
                .tab_index(if ready { 0 } else { -1 })
                .when(ready, |b| {
                    b.on_click(cx.listener(move |this, _, _, cx| {
                        if last {
                            this.guide_close(cx);
                        } else {
                            this.guide_go(GuideStep::ALL[current.index() + 1], cx);
                        }
                    }))
                });
        actions = actions.child(continue_button);
        row()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(10.))
            .pt(px(10.))
            .pb(px(4.))
            .child(
                div()
                    .id("cloud-guide-later")
                    .role(Role::Button)
                    .tab_index(0)
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(palette::rgb(MUTED))
                    .hover(|v| v.text_color(palette::rgb(TEXT)))
                    .child("Finish later")
                    .on_click(cx.listener(|this, _, _, cx| this.guide_close(cx))),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(palette::rgb(MUTED))
                    .when(!ready, |v| {
                        v.child(SharedString::from(waiting_copy(current)))
                    }),
            )
            .child(actions)
    }

    pub(super) fn render_guide(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let current = self.guide_current();
        let body = match current {
            GuideStep::Github => self.github_group(window, cx),
            GuideStep::Repositories => self.repositories(window, cx),
            GuideStep::Agent => self.render_agents(window, cx),
            GuideStep::Machine => self.render_machine(cx),
            GuideStep::Environment => self.render_environment(window, cx),
        };
        let checklist = self.guide_checklist(current, cx);
        let footer = self.guide_footer(current, cx);
        column()
            .gap(px(16.))
            .child(
                s::frame()
                    .child(self.guide_header())
                    .child(checklist)
                    .child(footer),
            )
            .child(body)
    }

    /// A compact, dismissable reminder above the normal panel.
    pub(super) fn guide_entry(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.loaded || self.owner.is_none() || !self.can_manage() {
            return None;
        }
        let complete = self.guide_required_done();
        Some(cs::group(
            "Setup",
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
                    false,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.guide_reopen(cx)))
                .into_any_element(),
            ),
            cs::setting_row(
                true,
                if complete {
                    "Cloud is ready"
                } else {
                    "Setup isn't finished"
                },
                if complete {
                    "GitHub, repositories and a coding agent are connected."
                } else {
                    "Finish the remaining steps before starting a cloud task."
                },
                None,
            ),
        ))
    }
}

fn waiting_copy(step: GuideStep) -> &'static str {
    match step {
        GuideStep::Github => "Waiting for a GitHub installation",
        GuideStep::Repositories => "Add at least one repository",
        GuideStep::Agent => "Connect one coding agent",
        GuideStep::Machine | GuideStep::Environment => "",
    }
}
