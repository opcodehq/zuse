//! What the right pane can add: the empty pane's launcher list and the tab
//! strip's `+` menu offer the same entries, in the same order.
use super::*;

/// One entry of the launcher and the `+` menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanelChoice {
    Files,
    /// The chat's pull request, when its branch has one.
    PullRequest,
    /// The review of the checkout's uncommitted changes (a main-area tab).
    Changes,
    Terminal,
    Browser,
    /// The commit graph. Offered from the `+` menu only.
    History,
}

impl PanelChoice {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::PullRequest => "PR",
            Self::Changes => "Changes",
            Self::Terminal => "Terminal",
            Self::Browser => "Browser",
            Self::History => "History",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Files => icons::FOLDER,
            Self::PullRequest => icons::PULL_REQUEST,
            Self::Changes => icons::GIT_COMPARE,
            Self::Terminal => icons::TERMINAL,
            Self::Browser => icons::GLOBE,
            Self::History => icons::GIT_BRANCH,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::PullRequest => "pull-request",
            Self::Changes => "changes",
            Self::Terminal => "terminal",
            Self::Browser => "browser",
            Self::History => "history",
        }
    }
}

/// The entries in display order: Files, PR, Changes, Terminal, Browser (and
/// History in the menu). Singletons already open are left out, as is PR
/// without a pull request and the git entries outside a repository.
pub(super) fn panel_choices(
    files_open: bool,
    pull_request: Option<bool>,
    git: bool,
    review_open: bool,
    menu: bool,
) -> Vec<PanelChoice> {
    let mut choices = Vec::new();
    if !files_open {
        choices.push(PanelChoice::Files);
    }
    if pull_request == Some(false) {
        choices.push(PanelChoice::PullRequest);
    }
    if git && !review_open {
        choices.push(PanelChoice::Changes);
    }
    choices.push(PanelChoice::Terminal);
    choices.push(PanelChoice::Browser);
    if git && menu {
        choices.push(PanelChoice::History);
    }
    choices
}

impl Shell {
    /// The selected chat's pull request URL, when its branch has one.
    fn chat_pull_request_url(&self, cx: &App) -> Option<String> {
        let state = self.state.read(cx);
        let chat = state.selected_chat_row()?;
        state
            .change_request_for_chat(chat)
            .map(|summary| summary.url.clone())
    }

    /// Whether a pull request tab with `url` is already in this panel's strip.
    fn chat_pull_request_open(&self, url: &str, cx: &App) -> bool {
        self.right_tabs
            .get(&self.panel_key(cx))
            .into_iter()
            .flatten()
            .any(|surface| match surface {
                RightSurface::PullRequest(id) => self
                    .chat_pull_requests
                    .get(id)
                    .is_some_and(|tab| tab.url == url),
                _ => false,
            })
    }

    fn panel_choices(&self, menu: bool, cx: &App) -> Vec<PanelChoice> {
        let key = self.panel_key(cx);
        let files_open = self
            .right_tabs
            .get(&key)
            .is_some_and(|tabs| tabs.contains(&RightSurface::Files));
        let pull_request = self
            .chat_pull_request_url(cx)
            .map(|url| self.chat_pull_request_open(&url, cx));
        let review_open = self.main_tab_rows(cx).iter().any(|row| row.review);
        panel_choices(
            files_open,
            pull_request,
            self.space_git_detected(cx),
            review_open,
            menu,
        )
    }

    fn panel_choice_shortcut(&self, choice: PanelChoice) -> Option<String> {
        (choice == PanelChoice::Terminal).then(|| {
            crate::settings::badge_combo(self.settings.keymap.get(ShortcutId::ToggleTerminal))
        })
    }

    pub(super) fn add_panel_choice(
        &mut self,
        choice: PanelChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match choice {
            PanelChoice::Files => self.add_files_surface(window, cx),
            PanelChoice::PullRequest => {
                if let Some(url) = self.chat_pull_request_url(cx) {
                    let device = self.chat_pull_request_device(None, cx);
                    self.open_chat_pull_request(url, device, window, cx);
                }
            }
            PanelChoice::Changes => self.open_review_tab(window, cx),
            PanelChoice::Terminal => self.add_terminal_surface(cx),
            PanelChoice::Browser => self.add_browser_surface(None, window, cx),
            PanelChoice::History => self.add_history_surface(window, cx),
        }
    }

    /// The empty pane: a centred column of card rows (icon, label, shortcut).
    pub(super) fn render_surface_picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let rows = self
            .panel_choices(false, cx)
            .into_iter()
            .map(|choice| {
                let fade = format!("surface-card-{}", choice.id());
                let shortcut = self.panel_choice_shortcut(choice);
                div()
                    .id(SharedString::from(fade.clone()))
                    .debug_selector(move || fade.clone())
                    .role(gpui::Role::Button)
                    .aria_label(choice.label())
                    .w_full()
                    .px(px(12.0))
                    .py(px(12.0))
                    .rounded(px(8.0))
                    .bg(theme.surface_card.opacity(0.8))
                    .hover(|style| style.bg(theme.surface_card.opacity(0.6)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .cursor_pointer()
                    .child(
                        icon(choice.icon())
                            .size(px(16.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::text_md())
                            .text_color(theme.text)
                            .child(SharedString::from(choice.label())),
                    )
                    .children(shortcut.map(|shortcut| {
                        div()
                            .flex_none()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted.opacity(0.7))
                            .child(SharedString::from(shortcut))
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_panel_choice(choice, window, cx)
                    }))
            })
            .collect::<Vec<_>>();
        div()
            .size_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .px(px(12.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(448.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(rows),
            )
            .into_any_element()
    }

    /// The `+` menu's card: the launcher's entries as menu rows.
    pub(super) fn render_right_plus_menu(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = theme.for_popup();
        let rows = self
            .panel_choices(true, cx)
            .into_iter()
            .map(|choice| {
                let fade = format!("right-plus-{}", choice.id());
                let shortcut = self.panel_choice_shortcut(choice);
                popover::menu_row(&theme, false, fade.clone())
                    .id(SharedString::from(format!("{fade}-row")))
                    .debug_selector(move || format!("{fade}-row"))
                    .role(gpui::Role::MenuItem)
                    .aria_label(choice.label())
                    .gap(px(10.0))
                    .text_size(crate::typography::text_md())
                    .child(
                        icon(choice.icon())
                            .size(px(14.0))
                            .flex_none()
                            .text_color(theme.text),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(choice.label())),
                    )
                    .children(shortcut.map(|shortcut| popover::kbd_hint(&theme, &shortcut)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_right_plus(cx);
                        this.add_panel_choice(choice, window, cx);
                    }))
            })
            .collect::<Vec<_>>();
        popover::popover_card(&theme)
            .w(px(288.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_right_plus(cx)))
            .child(div().flex().flex_col().children(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_follow_the_reference_order_and_hide_open_singletons() {
        use PanelChoice::*;
        assert_eq!(
            panel_choices(false, Some(false), true, false, false),
            [Files, PullRequest, Changes, Terminal, Browser]
        );
        // Each entry appears once, Files included.
        let menu = panel_choices(false, Some(false), true, false, true);
        assert_eq!(
            menu,
            [Files, PullRequest, Changes, Terminal, Browser, History]
        );
        // An open tree, PR or review is not offered again.
        assert_eq!(
            panel_choices(true, Some(true), true, true, true),
            [Terminal, Browser, History]
        );
        // No pull request, no repository: no PR or git entries.
        assert_eq!(
            panel_choices(false, None, false, false, true),
            [Files, Terminal, Browser]
        );
    }
}
