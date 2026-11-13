//! The Accounts tab: a provider's logins as one clean list — who, which plan,
//! how much of each usage window is left, and one click to switch — plus a
//! proper signed-out screen and the sign-in flow drawn in place.
use super::*;
use zuse_protocol::{
    AgentAccount, AgentLoginMode, AgentLoginPoll, AgentLoginStart, AgentLoginStatus,
};
use zuse_viewport::provider_signin::{login_command, login_options};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum LoginStep {
    Starting,
    Browser,
    Paste { submitting: bool },
    Failed(String),
}

#[derive(Clone, Debug)]
pub(super) struct Login {
    pub harness: HarnessId,
    pub provider: Option<&'static str>,
    pub login_id: Option<String>,
    pub url: Option<String>,
    pub message: Option<String>,
    pub step: LoginStep,
}

pub(super) struct AccountsState {
    pub busy: Option<String>,
    pub confirm_remove: Option<String>,
    pub error: Option<String>,
    pub login: Option<Login>,
    pub code: Entity<ComposerInput>,
    pub task: Option<Task<()>>,
    pub poll_task: Option<Task<()>>,
    pub refreshing: bool,
    pub _subscriptions: Vec<gpui::Subscription>,
}

impl AccountsState {
    pub fn new(cx: &mut Context<HarnessesPage>) -> Self {
        let code = cx.new(|cx| ComposerInput::new("Paste the code here", cx));
        let subscriptions = vec![
            cx.subscribe(&code, |page: &mut HarnessesPage, _, event, cx| {
                if matches!(event, ComposerInputEvent::Submitted) {
                    page.submit_login_code(cx);
                }
            }),
        ];
        Self {
            busy: None,
            confirm_remove: None,
            error: None,
            login: None,
            code,
            task: None,
            poll_task: None,
            refreshing: false,
            _subscriptions: subscriptions,
        }
    }
}

/// One line on what signing in gets you, per provider.
fn signin_pitch(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => {
            "Claude Code runs on your Claude plan. Add several subscriptions and switch whenever one runs low."
        }
        HarnessId::Codex => {
            "Codex runs on your ChatGPT plan. Add several accounts and switch whenever one runs low."
        }
        HarnessId::Cursor => {
            "Cursor runs on your Cursor plan. Zuse mints an API key you can revoke from Cursor's dashboard."
        }
        HarnessId::Grok => "Grok runs on your SuperGrok or X account.",
        HarnessId::Devin => "Devin runs on your Devin account.",
        HarnessId::Antigravity => "Antigravity runs on your Google account.",
        HarnessId::Hermes => "Hermes keeps its own pool of logins and rotates through them.",
        HarnessId::Opencode | HarnessId::Pi => "Sign in to a model provider to use its plan here.",
        _ => "Sign in to use this agent.",
    }
}

fn reset_in(at: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    let minutes = (at? - chrono::Utc::now()).num_minutes();
    Some(match minutes {
        m if m <= 0 => "resets now".into(),
        m if m < 60 => format!("resets in {m}m"),
        m if m < 48 * 60 => format!("resets in {}h", m / 60),
        m => format!("resets in {}d", m / (24 * 60)),
    })
}

impl HarnessesPage {
    fn accounts_call(
        &self,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<serde_json::Value, String>, &mut Context<Self>) + 'static,
    ) -> Option<Task<()>> {
        let engine = self.state.read(cx).engine().cloned()?;
        let params = self.with_target(params);
        Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string());
            this.update(cx, |page, cx| {
                done(page, result, cx);
                cx.notify();
            })
            .ok();
        }))
    }

    fn apply_snapshot(&mut self, result: Result<serde_json::Value, String>) -> bool {
        match result.and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string())) {
            Ok(snapshot) => {
                self.browser.accounts = Some(snapshot);
                self.logins.error = None;
                true
            }
            Err(error) => {
                self.logins.error = Some(error);
                false
            }
        }
    }

    /// Re-read logins and re-probe usage (the engine rate-limits probes).
    pub(super) fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        self.logins.refreshing = true;
        self.browser.accounts_task = self.accounts_call(
            methods::LIST_AGENT_ACCOUNTS,
            serde_json::json!({ "forceUsage": true }),
            cx,
            |page, result, _| {
                page.logins.refreshing = false;
                page.apply_snapshot(result);
            },
        );
        cx.notify();
    }

    fn account_action(
        &mut self,
        method: &'static str,
        account: &AgentAccount,
        cx: &mut Context<Self>,
    ) {
        if self.logins.busy.is_some() {
            return;
        }
        let id = account.id.clone();
        self.logins.busy = Some(id.clone());
        self.logins.confirm_remove = None;
        self.logins.task = self.accounts_call(
            method,
            serde_json::json!({ "harness": account.harness, "accountId": id, "id": id }),
            cx,
            |page, result, cx| {
                page.logins.busy = None;
                if page.apply_snapshot(result) {
                    // Sign-in state feeds the list dots and the composer.
                    page.load(cx);
                }
            },
        );
        cx.notify();
    }

    // ── sign-in ─────────────────────────────────────────────────────────────

    fn start_login(
        &mut self,
        harness: HarnessId,
        provider: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if self
            .logins
            .login
            .as_ref()
            .is_some_and(|l| !matches!(l.step, LoginStep::Failed(_)))
        {
            return;
        }
        self.logins.login = Some(Login {
            harness,
            provider,
            login_id: None,
            url: None,
            message: None,
            step: LoginStep::Starting,
        });
        self.logins.code.update(cx, |i, cx| i.set_text("", cx));
        let mut params = serde_json::json!({ "harness": harness });
        if let Some(provider) = provider {
            params["provider"] = serde_json::json!(provider);
        }
        self.logins.task = self.accounts_call(
            methods::START_AGENT_LOGIN,
            params,
            cx,
            |page, result, cx| {
                let Some(login) = page.logins.login.as_mut() else {
                    return;
                };
                match result.and_then(|v| {
                    serde_json::from_value::<AgentLoginStart>(v).map_err(|e| e.to_string())
                }) {
                    Ok(start) => {
                        if !start.url.is_empty() {
                            accounts::open_login_url(&start.url, cx);
                            login.url = Some(start.url.clone());
                        }
                        login.login_id = Some(start.login_id);
                        match start.mode {
                            AgentLoginMode::PasteCode => {
                                login.step = LoginStep::Paste { submitting: false }
                            }
                            AgentLoginMode::Browser => {
                                login.step = LoginStep::Browser;
                                page.poll_login(cx);
                            }
                        }
                    }
                    Err(error) => login.step = LoginStep::Failed(error),
                }
            },
        );
        cx.notify();
    }

    fn poll_login(&mut self, cx: &mut Context<Self>) {
        let Some(login_id) = self.logins.login.as_ref().and_then(|l| l.login_id.clone()) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({ "loginId": login_id }));
        self.logins.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                let result = engine
                    .client()
                    .call(methods::POLL_AGENT_LOGIN, params.clone())
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| {
                        serde_json::from_value::<AgentLoginPoll>(v).map_err(|e| e.to_string())
                    });
                let finished = this
                    .update(cx, |page, cx| {
                        let Some(login) = page.logins.login.as_mut() else {
                            return true;
                        };
                        let finished = match result {
                            Ok(poll) => {
                                if let Some(url) =
                                    poll.url.filter(|u| login.url.as_ref() != Some(u))
                                {
                                    accounts::open_login_url(&url, cx);
                                    login.url = Some(url);
                                }
                                login.message = poll.message.clone();
                                match poll.status {
                                    AgentLoginStatus::Pending => false,
                                    AgentLoginStatus::Done => {
                                        page.logins.login = None;
                                        page.refresh_usage(cx);
                                        page.load(cx);
                                        true
                                    }
                                    AgentLoginStatus::Error => {
                                        login.step =
                                            LoginStep::Failed(poll.message.unwrap_or_else(|| {
                                                "The sign-in didn't finish.".into()
                                            }));
                                        true
                                    }
                                }
                            }
                            Err(error) => {
                                login.step = LoginStep::Failed(error);
                                true
                            }
                        };
                        cx.notify();
                        finished
                    })
                    .unwrap_or(true);
                if finished {
                    return;
                }
            }
        }));
    }

    fn submit_login_code(&mut self, cx: &mut Context<Self>) {
        let Some(login) = self.logins.login.as_mut() else {
            return;
        };
        let (Some(login_id), LoginStep::Paste { submitting: false }) =
            (login.login_id.clone(), &login.step)
        else {
            return;
        };
        let code = self.logins.code.read(cx).text().trim().to_owned();
        if code.is_empty() {
            return;
        }
        login.step = LoginStep::Paste { submitting: true };
        self.logins.task = self.accounts_call(
            methods::COMPLETE_AGENT_LOGIN,
            serde_json::json!({ "loginId": login_id, "code": code }),
            cx,
            |page, result, cx| match result {
                Ok(value) => {
                    page.logins.login = None;
                    page.apply_snapshot(Ok(value));
                    page.load(cx);
                }
                Err(error) => {
                    if let Some(login) = page.logins.login.as_mut() {
                        login.step = LoginStep::Paste { submitting: false };
                        login.message = Some(error);
                    }
                }
            },
        );
        cx.notify();
    }

    pub(super) fn cancel_login(&mut self, cx: &mut Context<Self>) {
        self.logins.poll_task = None;
        if let Some(login_id) = self.logins.login.take().and_then(|l| l.login_id) {
            self.logins.task = self.accounts_call(
                methods::CANCEL_AGENT_LOGIN,
                serde_json::json!({ "loginId": login_id }),
                cx,
                |_, _, _| {},
            );
        }
        cx.notify();
    }

    // ── rendering ───────────────────────────────────────────────────────────

    pub(super) fn accounts_panel(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accounts: Vec<AgentAccount> = self
            .browser
            .accounts
            .as_ref()
            .map(|s| {
                s.accounts
                    .iter()
                    .filter(|a| a.harness == harness)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let login = self.logins.login.as_ref().filter(|l| l.harness == harness);
        if self.browser.accounts.is_none() {
            return div()
                .child(popover::skeleton_rows(
                    "accounts-skeleton",
                    theme,
                    2,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element();
        }
        if accounts.is_empty() {
            return match login {
                Some(login) => self.login_card(login, theme, cx),
                None => self.signed_out(harness, theme, cx),
            };
        }
        let warning = self
            .browser
            .accounts
            .as_ref()
            .and_then(|s| s.warnings.iter().find(|w| w.harness == harness))
            .map(|w| w.message.clone());
        let switches = accounts::switches_accounts(harness);
        let options = login_options(harness);
        let count = accounts.len();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child("Accounts"),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(count.to_string())),
                    ),
            )
            .child(
                div()
                    .id("accounts-refresh")
                    .size(px(28.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .role(gpui::Role::Button)
                    .aria_label("Refresh usage")
                    .tooltip(widgets::text_tooltip("Refresh usage"))
                    .hover(|s| s.bg(theme.settings_frame_bg()))
                    .when(self.logins.refreshing, |el| el.opacity(0.5))
                    .on_click(cx.listener(|page, _, _, cx| page.refresh_usage(cx)))
                    .child(
                        crate::icons::icon(crate::icons::REFRESH)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    ),
            )
            .when(!accounts::keeps_one_login(harness), |el| {
                el.children(options.into_iter().enumerate().map(|(ix, option)| {
                    let label = match option.provider {
                        Some(_) => format!("Add {}", option.label),
                        None => "Add account".to_owned(),
                    };
                    widgets::action_button(theme, widgets::ActionTone::Outlined)
                        .id(("accounts-add", ix))
                        .child(
                            crate::icons::icon(crate::icons::PLUS)
                                .size(px(12.))
                                .text_color(theme.text),
                        )
                        .child(SharedString::from(label))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.start_login(harness, option.provider, cx)
                        }))
                }))
            });
        let login_card = login.map(|login| self.login_card(login, theme, cx));
        let rows: Vec<AnyElement> = accounts
            .iter()
            .enumerate()
            .map(|(ix, account)| self.account_row(ix, account, switches, theme, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(header)
            .when_some(warning, |el, warning| {
                el.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.warning)
                        .child(SharedString::from(warning)),
                )
            })
            .when_some(accounts::provider_note(harness), |el, note| {
                el.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .child(note),
                )
            })
            .children(login_card)
            .child(widgets::section_card(theme).mt_0().children(rows))
            .when_some(self.logins.error.clone(), |el, error| {
                el.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.danger_muted)
                        .child(SharedString::from(error)),
                )
            })
            .when_some(login_command(harness), |el, command| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_size(px(11.))
                        .text_color(theme.text_muted)
                        .child("Logins made with")
                        .child(
                            div()
                                .px(px(5.))
                                .rounded(px(4.))
                                .bg(theme.settings_frame_bg())
                                .font_family(theme.font_mono.clone())
                                .child(command),
                        )
                        .child("in a terminal show up here too."),
                )
            })
            .into_any_element()
    }

    fn account_row(
        &self,
        ix: usize,
        account: &AgentAccount,
        switches: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let email = account.email.clone().unwrap_or_default();
        let title = account
            .display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| email.clone());
        let initial: String = title
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "?".into());
        let detail = (!email.is_empty() && email != title)
            .then(|| zuse_protocol::presentation::masked_email(&email));
        let busy = self.logins.busy.as_deref() == Some(account.id.as_str());
        let confirming = self.logins.confirm_remove.as_deref() == Some(account.id.as_str());
        let id = account.id.clone();
        let for_use = account.clone();
        let for_remove = account.clone();

        // Usage: one tiny meter per window, the reset time on hover.
        let meters = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.))
            .children(account.usage_windows.iter().enumerate().map(|(wix, w)| {
                let used = w.used_fraction.clamp(0.0, 1.0);
                let color = if used >= 0.9 {
                    theme.danger
                } else if used >= 0.7 {
                    theme.warning
                } else {
                    theme.success
                };
                let short = match w.label.to_lowercase().as_str() {
                    l if l.contains("5") || l.contains("session") => "5h".to_owned(),
                    l if l.contains("week") => "Week".to_owned(),
                    _ => w.label.clone(),
                };
                let tip = match reset_in(w.resets_at) {
                    Some(reset) => format!(
                        "{} · {}% used · {reset}",
                        w.label,
                        (used * 100.0).round() as i32
                    ),
                    None => format!("{} · {}% used", w.label, (used * 100.0).round() as i32),
                };
                div()
                    .id(SharedString::from(format!("usage-{ix}-{wix}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .tooltip(widgets::text_tooltip(tip))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(short)),
                    )
                    .child(
                        div()
                            .w(px(48.))
                            .h(px(4.))
                            .rounded_full()
                            .bg(theme.settings_frame_bg())
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(color)
                                    .w(gpui::relative(used.max(0.03))),
                            ),
                    )
                    .child(
                        div()
                            .w(px(28.))
                            .text_size(px(10.5))
                            .text_color(if used >= 0.9 {
                                theme.danger_muted
                            } else {
                                theme.text
                            })
                            .child(SharedString::from(format!(
                                "{}%",
                                (used * 100.0).round() as i32
                            ))),
                    )
            }));
        let usage_note = (account.usage_windows.is_empty())
            .then(|| account.usage_error.clone())
            .flatten();

        let mut actions = div().flex_none().flex().items_center().gap(px(4.));
        if confirming {
            actions = actions
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .mr(px(4.))
                        .child("Remove?"),
                )
                .child(
                    widgets::text_action(theme, widgets::ActionTone::Quiet, "Cancel")
                        .id(("account-remove-cancel", ix))
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.logins.confirm_remove = None;
                            cx.notify();
                        })),
                )
                .child(
                    widgets::text_action(theme, widgets::ActionTone::Danger, "Remove")
                        .id(("account-remove-confirm", ix))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.account_action(methods::FORGET_AGENT_ACCOUNT, &for_remove, cx)
                        })),
                );
        } else {
            let status: AnyElement = if account.active {
                widgets::badge_active(theme, "In use").into_any_element()
            } else if switches && account.switchable {
                div()
                    .id(("account-use", ix))
                    .px(px(10.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(theme.text)
                    .hover(|s| s.bg(theme.settings_frame_bg()))
                    .when(busy, |el| el.opacity(0.6))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.account_action(methods::ACTIVATE_AGENT_ACCOUNT, &for_use, cx)
                    }))
                    .child(if busy { "Switching…" } else { "Use" })
                    .into_any_element()
            } else {
                div().into_any_element()
            };
            actions = actions.child(div().w(px(64.)).flex().justify_end().child(status));
            if switches && account.switchable {
                actions = actions.child(
                    div()
                        .id(("account-remove", ix))
                        .size(px(24.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .role(gpui::Role::Button)
                        .aria_label("Remove login")
                        .tooltip(widgets::text_tooltip("Remove login"))
                        .hover(|s| s.bg(theme.settings_frame_bg()))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.logins.confirm_remove = Some(id.clone());
                            cx.notify();
                        }))
                        .child(
                            crate::icons::icon(crate::icons::TRASH_BIN_MINIMALISTIC)
                                .size(px(13.))
                                .text_color(theme.text_muted),
                        ),
                );
            }
        }

        div()
            .px(px(12.))
            .h(px(48.))
            .when(ix > 0, |el| {
                el.border_t_1().border_color(widgets::row_divider(theme))
            })
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .flex_none()
                    .size(px(26.))
                    .rounded_full()
                    .bg(if account.active {
                        theme.success.opacity(0.16)
                    } else {
                        theme.settings_frame_bg()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(if account.active {
                        theme.success_muted
                    } else {
                        theme.text
                    })
                    .child(SharedString::from(initial)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(widgets::row_title(theme, title).flex_none())
                    .when_some(account.plan_label.clone(), |el, plan| {
                        el.child(widgets::badge(theme, plan))
                    })
                    .when_some(detail, |el, detail| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(detail)),
                        )
                    }),
            )
            .child(meters)
            .when_some(usage_note, |el, note| {
                el.child(
                    div()
                        .flex_none()
                        .max_w(px(200.))
                        .truncate()
                        .text_size(px(11.))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(note)),
                )
            })
            .child(actions)
            .into_any_element()
    }

    fn signed_out(&self, harness: HarnessId, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let name = accounts::provider_name(harness);
        let (icon_path, tint) = crate::pickers::harness_brand_icon(harness);
        let options = login_options(harness);
        let primary = options.len() == 1;
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(14.))
            .py(px(36.))
            .px(px(24.))
            .rounded(px(12.))
            .border_1()
            .border_color(widgets::row_divider(theme))
            .bg(theme.settings_frame_bg().opacity(0.25))
            .child(
                div()
                    .size(px(48.))
                    .rounded(px(12.))
                    .bg(theme.settings_frame_bg())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::icons::brand_icon(
                        icon_path,
                        26.,
                        tint.unwrap_or(theme.text),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_strong)
                            .child(SharedString::from(format!("Sign in to use {name}"))),
                    )
                    .child(
                        div()
                            .max_w(px(420.))
                            .text_center()
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child(signin_pitch(harness)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(8.))
                    .children(options.into_iter().enumerate().map(|(ix, option)| {
                        let label = match option.provider {
                            Some(_) => format!("Sign in with {}", option.label),
                            None => format!("Sign in to {name}"),
                        };
                        widgets::text_action(
                            theme,
                            if primary || ix == 0 {
                                widgets::ActionTone::Filled
                            } else {
                                widgets::ActionTone::Outlined
                            },
                            label,
                        )
                        .id(("signin-start", ix))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.start_login(harness, option.provider, cx)
                        }))
                    })),
            )
            .when_some(login_command(harness), |el, command| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(theme.text_muted)
                        .child("or run")
                        .child(
                            div()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(5.))
                                .bg(theme.settings_frame_bg())
                                .font_family(theme.font_mono.clone())
                                .child(command),
                        )
                        .child("in a terminal"),
                )
            })
            .when_some(self.logins.error.clone(), |el, error| {
                el.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.danger_muted)
                        .child(SharedString::from(error)),
                )
            })
            .into_any_element()
    }

    fn login_card(&self, login: &Login, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let failed = matches!(login.step, LoginStep::Failed(_));
        let paste = matches!(login.step, LoginStep::Paste { .. });
        let submitting = matches!(login.step, LoginStep::Paste { submitting: true });
        let text: String = match &login.step {
            LoginStep::Starting => "Opening the sign-in page…".into(),
            LoginStep::Browser => login
                .message
                .clone()
                .unwrap_or_else(|| "Waiting for you to sign in in your browser…".into()),
            LoginStep::Paste { .. } => "Paste the code from your browser".into(),
            LoginStep::Failed(error) => error.clone(),
        };
        let harness = login.harness;
        let provider = login.provider;
        let url = login.url.clone().filter(|_| !failed);
        let link = |label: &'static str, id: &'static str| {
            div()
                .id(id)
                .flex_none()
                .px(px(6.))
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(theme.text_muted)
                .hover(|s| s.text_color(theme.text).bg(theme.settings_frame_bg()))
                .child(label)
        };
        div()
            .px(px(12.))
            .py(px(8.))
            .min_h(px(40.))
            .rounded(px(8.))
            .bg(if failed {
                theme.danger.opacity(0.07)
            } else {
                theme.settings_frame_bg().opacity(0.55)
            })
            .flex()
            .items_center()
            .gap(px(10.))
            .child(if failed {
                crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                    .size(px(14.))
                    .text_color(theme.danger_muted)
                    .into_any_element()
            } else if paste {
                crate::icons::icon(crate::icons::INFO_CIRCLE)
                    .size(px(14.))
                    .text_color(theme.text_muted)
                    .into_any_element()
            } else {
                crate::loaders::mini_mono_spinner(
                    "account-login-spinner",
                    2.0,
                    theme.text_muted,
                    cx.entity_id(),
                    cx,
                )
                .into_any_element()
            })
            .child(
                div()
                    .when(!paste, |el| el.flex_1())
                    .when(paste, |el| el.flex_none())
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(if failed {
                        theme.danger_muted
                    } else {
                        theme.text
                    })
                    .child(SharedString::from(text)),
            )
            .when(paste, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_w(px(120.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.input_glass_bg())
                        .font_family(theme.font_mono.clone())
                        .text_size(px(12.))
                        .child(self.logins.code.clone()),
                )
                .child(
                    link(if submitting { "Checking…" } else { "Add" }, "login-submit")
                        .text_color(theme.text_strong)
                        .on_click(cx.listener(|page, _, _, cx| page.submit_login_code(cx))),
                )
            })
            .when_some(url, |el, url| {
                el.child(
                    link("Reopen page", "login-reopen")
                        .on_click(move |_, _, cx| accounts::open_login_url(&url, cx)),
                )
            })
            .when(failed, |el| {
                el.child(
                    link("Try again", "login-retry")
                        .text_color(theme.text)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.logins.login = None;
                            page.start_login(harness, provider, cx)
                        })),
                )
            })
            .child(
                link(if failed { "Dismiss" } else { "Cancel" }, "login-cancel")
                    .on_click(cx.listener(|page, _, _, cx| page.cancel_login(cx))),
            )
            .into_any_element()
    }
}
