//! Settings → Providers: install and enable harnesses, one card row per agent.
//!
//! The state is PER-DEVICE and lives on the engine (`harness-prefs.json` in
//! its data dir): CLI installs are per-device, so enablement is too. The page
//! shows this device; the shell's update notice can retarget both the
//! `ListHarnesses` probe and the `SetHarnessEnabled` writes at another
//! registered device over the relay-forwarded RPCs (the header then names
//! it). There is no per-page device switcher.
//!
//! Enablement follows DETECTION: every harness whose CLI probe passes is on
//! unless the user switched it off, so installing an agent is all it takes
//! for it to appear here and in the composer. A harness whose CLI is missing
//! on the target device renders dimmed with an install hint and is never
//! enabled (enabling an agent that can't run would only manufacture
//! NotInstalled errors at send time); an ENABLED agent can always be turned
//! OFF except the last one standing — the composer needs something to run.
//! Catalogs from engines predating the detection model can still stamp
//! enabled-but-uninstalled rows; the hint covers that too. The engine
//! enforces the same gates where the state lives, so a raced or stale toggle
//! self-corrects from the RPC reply.

use crate::localization::message as tr;
use crate::tooltip::TooltipExt as _;
use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Task, Window, div, prelude::*,
    px,
};
use zuse_i18n::messages as msg;

use zuse_engine::registry::{HarnessDescriptor, descriptor_enabled};

use zuse_protocol::{HarnessId, HarnessUpdatePhase, HarnessUpdatePolicy, HarnessUpdateStatus};

use zuse_rpc::methods;

use crate::motion;
use crate::pickers::visible_harnesses;
use crate::popover::{self, Loadable};
use crate::settings::accounts;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

#[path = "accounts_panel.rs"]
mod accounts_panel;
#[path = "acp_setup.rs"]
mod acp_setup;
#[path = "completion.rs"]
mod completion;
#[path = "provider_browser.rs"]
mod provider_browser;

use crate::composer::{ComposerInput, ComposerInputEvent};

/// Update policies in menu order: the short label, and the note shown under
/// the row title for the chosen one.
const UPDATE_POLICIES: [(HarnessUpdatePolicy, &str, &str); 3] = [
    (
        HarnessUpdatePolicy::Notify,
        "Notify",
        "Install only when you choose Update.",
    ),
    (
        HarnessUpdatePolicy::AutoWhenIdle,
        "Auto when idle",
        "Install automatically after active runs finish.",
    ),
    (
        HarnessUpdatePolicy::Off,
        "Off",
        "Don't check for new versions.",
    ),
];

fn offers_install(harness: HarnessId, installed: bool, can_install: bool) -> bool {
    harness != HarnessId::Mock && !installed && can_install
}

fn install_hint(harness: HarnessId, enabled: bool, can_install: bool) -> String {
    if harness == HarnessId::Antigravity {
        return if can_install {
            "Install Antigravity to enable"
        } else {
            "Set ANTIGRAVITY_ACP_EXECUTABLE to enable Antigravity"
        }
        .into();
    }
    let hint = if enabled {
        format!(
            "{} CLI not installed — turn it off or install it",
            cli_name(harness)
        )
    } else {
        format!("Install the {} CLI to enable", cli_name(harness))
    };
    if !can_install && let Some(command) = zuse_providers::install::manual_command(harness) {
        format!("{hint}. Install with `{command}`")
    } else {
        hint
    }
}

fn install_label(name: &str) -> String {
    format!("Installing {name}…")
}

fn install_params(harness: HarnessId, target: &Option<String>) -> serde_json::Value {
    serde_json::json!({"harness": harness, "targetDeviceId": target})
}

/// The CLI named in the not-installed hint.
pub fn cli_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "claude",
        HarnessId::Codex => "codex",
        HarnessId::Cursor => "cursor-agent",
        HarnessId::Devin => "devin",
        HarnessId::Grok => "grok",
        HarnessId::Hermes => "hermes",
        HarnessId::Pi => "pi",
        HarnessId::Muse => "muse",
        HarnessId::Opencode => "opencode",
        HarnessId::Antigravity => "Antigravity",
        HarnessId::Gemini => "gemini",
        HarnessId::Kiro => "kiro-cli",
        HarnessId::Zuse => "Zuse",
        HarnessId::Mock => "mock",
        HarnessId::CustomAcp(_) => "configured ACP executable",
    }
}

/// The tone of a provider row's status dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusTone {
    Ready,
    Warning,
    Error,
    Muted,
}

impl StatusTone {
    fn color(self, theme: &Theme) -> gpui::Hsla {
        match self {
            StatusTone::Ready => theme.success,
            StatusTone::Warning => theme.warning,
            StatusTone::Error => theme.danger,
            StatusTone::Muted => theme.text_muted.opacity(0.6),
        }
    }
}

/// What an update adds to the status line, beyond the version shown next to
/// the name: an available release, progress, or a failure. Up-to-date
/// providers and checks in flight add nothing, so rows don't flash through
/// states while checks run.
fn update_detail(status: &HarnessUpdateStatus) -> Option<String> {
    let latest = status
        .latest_version
        .as_deref()
        .filter(|latest| status.installed_version.as_deref() != Some(*latest));
    match status.phase {
        HarnessUpdatePhase::Dormant
        | HarnessUpdatePhase::Checking
        | HarnessUpdatePhase::Current
        | HarnessUpdatePhase::ManualActionRequired => None,
        HarnessUpdatePhase::Available => Some(match latest {
            Some(latest) => format!("v{latest} available"),
            None => "Update available".into(),
        }),
        HarnessUpdatePhase::WaitingForIdle => Some("Waiting for agent to be idle".into()),
        HarnessUpdatePhase::Preparing => Some("Preparing update…".into()),
        HarnessUpdatePhase::Downloading => Some("Downloading…".into()),
        HarnessUpdatePhase::Installing => Some("Installing…".into()),
        HarnessUpdatePhase::Verifying => Some("Verifying…".into()),
        HarnessUpdatePhase::Updated => Some("Updated".into()),
        HarnessUpdatePhase::Failed => Some("Couldn't check for updates".into()),
    }
}

/// A provider row's status line: ready, off, not installed (with the install
/// hint) or a failed update check, plus any update detail.
fn provider_status(
    harness: HarnessId,
    installed: bool,
    enabled: bool,
    can_install: bool,
    update: Option<&HarnessUpdateStatus>,
) -> (StatusTone, String) {
    if !installed {
        return (
            StatusTone::Warning,
            install_hint(harness, enabled, can_install),
        );
    }
    let detail = update.and_then(update_detail);
    if update.is_some_and(|status| status.phase == HarnessUpdatePhase::Failed) {
        return (StatusTone::Error, detail.unwrap_or_default());
    }
    let (tone, label) = if enabled {
        (StatusTone::Ready, "Ready")
    } else {
        (StatusTone::Muted, "Off")
    };
    match detail {
        Some(detail) => (tone, format!("{label} · {detail}")),
        None => (tone, label.into()),
    }
}

/// The frame's "Checked 2m ago" from the newest update check (epoch ms).
fn checked_label(checked_at: Option<i64>, now_ms: i64) -> String {
    let Some(checked_at) = checked_at else {
        return "Checked".into();
    };
    let minutes = (now_ms - checked_at).max(0) / 60_000;
    match minutes {
        0 => "Checked just now".into(),
        1..=59 => format!("Checked {minutes}m ago"),
        60..=1439 => format!("Checked {}h ago", minutes / 60),
        _ => format!("Checked {}d ago", minutes / 1440),
    }
}

pub struct HarnessesPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    updates: Loadable<Vec<HarnessUpdateStatus>>,
    /// Which device's harnesses are shown/edited; `None` = this device (no
    /// passthrough). Retargeted by the shell's harness update notice.
    target_device: Option<String>,
    policy_selects: std::collections::HashMap<HarnessId, widgets::SelectState>,
    /// Last refused/failed toggle (engine guards), shown in the error strip.
    error: Option<String>,
    load_task: Option<Task<()>>,
    toggle_task: Option<Task<()>>,
    /// A toggle the engine hasn't answered yet: the switch shows it at once.
    pending_toggle: Option<(HarnessId, bool)>,
    installing: Option<HarnessId>,
    install_task: Option<Task<()>>,

    expanded_harness: Option<HarnessId>,
    open_options: std::collections::HashSet<HarnessId>,
    /// Active account section; each computer/provider retains its own login owner.
    update_task: Option<Task<()>>,
    update_action_task: Option<Task<()>>,
    /// A manual provider refresh is checking available CLI updates.
    checking_updates: bool,
    /// The list/detail browser's tabs, models and Claude connection.
    browser: provider_browser::BrowserState,
    /// Adding and editing ACP agents.
    acp: acp_setup::AcpState,
    /// Select this provider once the next catalog lands (a just-added agent).
    pending_select: Option<HarnessId>,
    /// The Accounts tab's actions and in-place sign-in.
    logins: accounts_panel::AccountsState,
    /// Usage was re-probed once since the page opened.
    usage_probed: bool,
}

impl HarnessesPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            harnesses: Loadable::Idle,
            updates: Loadable::Idle,
            target_device: None,
            policy_selects: Default::default(),
            error: None,
            load_task: None,
            toggle_task: None,
            pending_toggle: None,
            installing: None,
            install_task: None,

            expanded_harness: None,
            open_options: Default::default(),
            update_task: None,
            update_action_task: None,
            checking_updates: false,
            browser: provider_browser::BrowserState::new(cx),
            acp: acp_setup::AcpState::new(cx),
            pending_select: None,
            logins: accounts_panel::AccountsState::new(cx),
            usage_probed: false,
        };
        page.load(cx);
        page
    }

    pub(crate) fn refresh_on_visit(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    /// Sign in row for agents whose own CLI owns sign-in (no Accounts section):
    /// the login command to copy, and how to store an API key instead.
    fn render_cli_sign_in(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use zuse_viewport::provider_signin::{login_command, login_hint};
        let hint = login_hint(harness, cx)?;
        let command = login_command(harness)?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(tr(cx, msg::common::signIn)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(7.))
                        .bg(theme.settings_frame_bg().opacity(0.4))
                        .px(px(10.))
                        .py(px(6.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(theme.font_mono.clone())
                                .text_size(px(11.))
                                .child(command),
                        )
                        .child(
                            widgets::action_button(theme, widgets::ActionTone::Quiet)
                                .id("copy-provider-login")
                                .aria_label(tr(
                                    cx,
                                    msg::settings::cloud_workspace_auth_copy_command,
                                ))
                                .role(gpui::Role::Button)
                                .tab_index(0)
                                .size(px(24.))
                                .px_0()
                                .justify_center()
                                .child(
                                    crate::icons::icon(crate::icons::COPY)
                                        .size(px(12.))
                                        .text_color(theme.text_muted),
                                )
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        command.to_owned(),
                                    ))
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_muted)
                        .child(hint),
                )
                .into_any_element(),
        )
    }

    /// The expanded provider's Updates section: the update policy, short
    /// labels in the menu and the chosen one explained under the title.
    fn render_updates_for(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Loadable::Ready(statuses) = &self.updates else {
            return None;
        };
        let status = statuses.iter().find(|status| status.harness == harness)?;
        let selected = UPDATE_POLICIES
            .iter()
            .position(|(policy, ..)| *policy == status.policy)
            .unwrap_or(0);
        let closed = widgets::SelectState::default();
        let policy_select = widgets::select(
            format!("harness-update-policy-{harness:?}"),
            "Update policy",
            theme,
            move |page: &mut Self| page.policy_selects.entry(harness).or_default(),
        )
        .options(
            UPDATE_POLICIES
                .iter()
                .map(|(_, label, _)| widgets::SelectOption::new(*label)),
            selected,
        )
        // Fits the longest label, so switching never resizes the trigger.
        .width(128.0)
        .on_select(move |page, selected, _, cx| {
            if let Some((policy, ..)) = UPDATE_POLICIES.get(selected) {
                page.set_update_policy(harness, *policy, cx);
            }
        })
        .render(self.policy_selects.get(&harness).unwrap_or(&closed), cx);
        let row = widgets::details_row(theme, true)
            .child(widgets::details_text(
                theme,
                "Update policy",
                UPDATE_POLICIES[selected].2,
            ))
            .child(policy_select);
        Some(
            div()
                .flex()
                .flex_col()
                .child(widgets::details_label(theme, "Updates"))
                .child(row)
                .into_any_element(),
        )
    }

    /// Params with the `targetDeviceId` passthrough merged in.
    fn with_target(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let (Some(target), Some(object)) = (&self.target_device, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    /// Escape that reached Settings unclaimed goes to the expanded provider's
    /// accounts (an open login) first. Returns whether it was consumed.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.acp.open {
            self.close_acp(cx);
            return true;
        }
        if self.logins.login.is_some() {
            self.cancel_login(cx);
            return true;
        }
        false
    }

    fn supports_updates(&self, cx: &Context<Self>) -> bool {
        let state = self.state.read(cx);
        let Some(engine) = state.engine() else {
            return false;
        };
        self.target_device.as_deref().map_or_else(
            || {
                engine
                    .engine_info()
                    .supports(zuse_protocol::capabilities::HARNESS_UPDATES_V1)
            },
            |device| state.device_supports(device, zuse_protocol::capabilities::HARNESS_UPDATES_V1),
        )
    }

    fn can_control_updates(&self, cx: &Context<Self>) -> bool {
        self.supports_updates(cx)
            && matches!(self.updates, Loadable::Ready(_))
            && self.target_device.as_ref().is_none_or(|device| {
                self.state
                    .read(cx)
                    .device_online(device, chrono::Utc::now())
            })
    }

    /// Retarget the page at another device: a different device is a different
    /// install/enablement world, so drop the rows and reload through it.

    pub(crate) fn set_target_device(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        if self.target_device == target {
            cx.notify();
            return;
        }
        self.installing = None;
        self.install_task = None;
        self.policy_selects.clear();
        self.open_options.clear();
        self.target_device = target;
        self.acp.snapshot = None;
        self.expanded_harness = None;
        self.error = None;
        self.harnesses = Loadable::Idle;
        self.updates = Loadable::Idle;
        self.update_task = None;
        self.update_action_task = None;
        self.load(cx);
        cx.notify();
    }

    /// `ListHarnesses` against the target device (installed probe + enabled
    /// set both come from where the CLIs actually live).
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({}));
        let update_params = params.clone();
        let supports_updates = self.supports_updates(cx);

        self.harnesses = Loadable::Loading;
        self.updates = if supports_updates {
            Loadable::Loading
        } else {
            Loadable::Ready(Vec::new())
        };
        self.update_task = supports_updates.then(|| {
            let update_engine = engine.clone();
            cx.spawn(async move |this, cx| {
                let mut retry = 1;
                loop {
                    let subscribed = update_engine
                        .client()
                        .subscribe_checked(methods::WATCH_HARNESS_UPDATES, update_params.clone())
                        .await;
                    match subscribed {
                        Ok(mut stream) => {
                            while let Some(value) = stream.recv().await {
                                retry = 1;
                                let parsed =
                                    serde_json::from_value::<Vec<HarnessUpdateStatus>>(value)
                                        .map_err(|error| error.to_string());
                                if this
                                    .update(cx, |page, cx| {
                                        page.updates = match parsed {
                                            Ok(statuses) => Loadable::Ready(statuses),
                                            Err(error) => Loadable::Error(error),
                                        };
                                        cx.notify();
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                        Err(error) => {
                            this.update(cx, |page, cx| {
                                page.updates = Loadable::Error(error.to_string());
                                cx.notify();
                            })
                            .ok();
                        }
                    }
                    if this
                        .update(cx, |page, cx| {
                            if !matches!(page.updates, Loadable::Error(_)) {
                                page.updates = Loadable::Error(
                                    "Connection closed. Reconnecting to device…".into(),
                                );
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(retry))
                        .await;
                    retry = (retry * 2).min(15);
                }
            })
        });
        self.load_browser_accounts(cx);
        self.load_acp_agents(cx);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::LIST_HARNESSES, params).await;
            this.update(cx, |page, cx| {
                page.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                if let Some(id) = page.pending_select.take() {
                    page.select_provider(id, cx);
                }
                page.default_selection(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Flip one harness on the target device. The reply carries the device's
    /// fresh catalog, so the rows repaint from the authoritative state in one
    /// round trip; refusals (engine guards) land in the error strip.
    fn toggle(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        self.set_enabled(harness, enabled, cx);
    }

    fn cancel_install(&mut self, cx: &mut Context<Self>) {
        let Some(harness) = self.installing else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target_device.clone();
        let params = install_params(harness, &target);
        cx.spawn(async move |this, cx| {
            if let Err(error) = engine.client().call(methods::CANCEL_INSTALL, params).await {
                this.update(cx, |page, cx| {
                    if page.target_device == target && page.installing == Some(harness) {
                        page.error = Some(format!("Cancellation failed — {error}"));
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn install(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if self.installing.is_some() {
            return;
        }
        let params = install_params(harness, &self.target_device);
        let target = self.target_device.clone();
        self.installing = Some(harness);
        self.error = None;
        self.install_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::INSTALL_HARNESS, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                if page.target_device != target {
                    return;
                }
                page.installing = None;
                match result {
                    Ok(list) => {
                        page.harnesses = Loadable::Ready(list);
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(error) => page.error = Some(format!("Installation failed — {error}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_enabled(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({
            "harness": harness,
            "enabled": enabled,
        }));
        let target = self.target_device.clone();
        self.error = None;
        self.pending_toggle = Some((harness, enabled));
        cx.notify();
        self.toggle_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_HARNESS_ENABLED, params)
                .await;
            this.update(cx, |page, cx| {
                if page.target_device != target {
                    return;
                }
                if page.pending_toggle == Some((harness, enabled)) {
                    page.pending_toggle = None;
                }
                match result {
                    Ok(value) => {
                        if let Ok(list) = serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                            page.harnesses = Loadable::Ready(list);
                        }
                        if !enabled && page.expanded_harness == Some(harness) {
                            page.expanded_harness = None;
                        }
                        // The composer caches its catalog per space — poke
                        // every Pickers to re-fetch, or the rail keeps the
                        // old set until restart.
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn check_updates(&mut self, cx: &mut Context<Self>) {
        if !self.can_control_updates(cx) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({}));
        self.error = None;
        self.checking_updates = true;
        cx.notify();
        self.update_action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CHECK_HARNESS_UPDATES, params)
                .await;
            this.update(cx, |page, cx| {
                page.checking_updates = false;
                match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessUpdateStatus>>(value) {
                        Ok(statuses) => page.updates = Loadable::Ready(statuses),
                        Err(error) => page.error = Some(error.to_string()),
                    },
                    Err(error) => page.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn apply_harness_update(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if !self.can_control_updates(cx) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({ "harness": harness }));
        self.error = None;
        self.update_action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::APPLY_HARNESS_UPDATE, params)
                .await;
            this.update(cx, |page, cx| {
                if let Err(error) = result {
                    page.error = Some(error.to_string());
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn cancel_harness_update(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if !self.can_control_updates(cx) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({ "harness": harness }));
        self.update_action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CANCEL_HARNESS_UPDATE, params)
                .await;
            this.update(cx, |page, cx| {
                if let Err(error) = result {
                    page.error = Some(error.to_string());
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_update_policy(
        &mut self,
        harness: HarnessId,
        policy: HarnessUpdatePolicy,
        cx: &mut Context<Self>,
    ) {
        if !self.can_control_updates(cx) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({
            "harness": harness,
            "policy": policy,
        }));
        self.error = None;
        self.update_action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_HARNESS_UPDATE_POLICY, params)
                .await;
            this.update(cx, |page, cx| {
                if let Err(error) = result {
                    page.error = Some(error.to_string());
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Review fixtures: a seeded provider list; `acp` opens the Add ACP
    /// agent dialog, `expanded` expands the first provider.
    #[cfg(feature = "browser-fixture")]
    pub fn fixture_seed(&mut self, variant: &str, cx: &mut Context<Self>) {
        let descriptor = |id, name: &str, installed: bool, enabled: bool| HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: false,
            steering_mode: zuse_protocol::SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed,
            can_install: true,
            enabled: Some(enabled),
            signed_in: None,
        };
        self.harnesses = Loadable::Ready(vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code", true, true),
            descriptor(HarnessId::Codex, "Codex", true, true),
            descriptor(HarnessId::Cursor, "Cursor", true, false),
            descriptor(HarnessId::Gemini, "Gemini CLI", false, false),
            descriptor(HarnessId::Opencode, "OpenCode", true, true),
        ]);
        let now = chrono::Utc::now().timestamp_millis();
        let status = |harness, installed: &str, latest: Option<&str>, phase| HarnessUpdateStatus {
            installed_version: Some(installed.into()),
            latest_version: latest.map(str::to_owned),
            checked_at: Some(now - 120_000),
            phase,
            can_apply: true,
            ..serde_json::from_value(serde_json::json!({"harness": harness, "phase": "current"}))
                .expect("fixture update status")
        };
        self.updates = Loadable::Ready(vec![
            status(
                HarnessId::ClaudeCode,
                "2.1.14",
                Some("2.1.15"),
                HarnessUpdatePhase::Available,
            ),
            status(
                HarnessId::Codex,
                "0.48.0",
                Some("0.49.1"),
                HarnessUpdatePhase::Available,
            ),
            status(
                HarnessId::Cursor,
                "2025.10.2",
                None,
                HarnessUpdatePhase::Current,
            ),
            status(
                HarnessId::Opencode,
                "1.0.12",
                None,
                HarnessUpdatePhase::Current,
            ),
        ]);
        if variant == "expanded" {
            self.select_provider(HarnessId::ClaudeCode, cx);
        }
        if variant == "acp" {
            self.open_acp(cx);
        }
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for HarnessesPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for HarnessesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let body: gpui::AnyElement = match &self.harnesses {
            Loadable::Idle | Loadable::Loading => widgets::section_card(&theme)
                .mt_0()
                .p(px(16.0))
                .child(popover::skeleton_rows(
                    "harnesses-skeleton",
                    &theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Loadable::Error(message) => {
                let message = message.clone();
                div()
                    .child(widgets::error_strip(&theme, message))
                    .child(
                        widgets::ghost_action(&theme)
                            .id("harnesses-retry")
                            .mt(px(8.0))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(cx);
                                cx.notify();
                            }))
                            .child(SharedString::from("Retry")),
                    )
                    .into_any_element()
            }
            Loadable::Ready(_) => self.render_browser(&theme, cx),
        };
        crate::toast::take_error(&mut self.error, cx);
        let update_error = match &self.updates {
            Loadable::Error(message) => Some(
                div()
                    .child(widgets::error_strip(
                        &theme,
                        format!("Agent updates: {message}"),
                    ))
                    .child(
                        widgets::ghost_action(&theme)
                            .id("harness-updates-retry")
                            .mt(px(8.0))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(cx);
                                cx.notify();
                            }))
                            .child(tr(cx, msg::native::retry_updates)),
                    )
                    .into_any_element(),
            ),
            _ => None,
        };
        // No device switcher: the page is this device. A remote target (only
        // reachable from the shell's update notice) is named in the status.
        let remote_device: Option<SharedString> = self.target_device.as_ref().map(|id| {
            self.state
                .read(cx)
                .devices
                .iter()
                .find(|device| &device.id == id)
                .map(|device| device.name.clone())
                .unwrap_or_else(|| "Another device".into())
                .into()
        });

        let checking =
            matches!(self.harnesses, Loadable::Idle | Loadable::Loading) || self.checking_updates;
        let status: SharedString = if checking {
            "Checking…".into()
        } else if matches!(self.harnesses, Loadable::Error(_)) {
            "Probe failed".into()
        } else {
            let checked_at = match &self.updates {
                Loadable::Ready(statuses) => statuses.iter().filter_map(|s| s.checked_at).max(),
                _ => None,
            };
            checked_label(checked_at, chrono::Utc::now().timestamp_millis()).into()
        };
        let status = match remote_device {
            Some(device) => format!("{device} · {status}").into(),
            None => status,
        };
        let check = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.text_muted)
                    .child(status),
            )
            .child(
                div()
                    .id("refresh-provider-status")
                    .role(gpui::Role::Button)
                    .aria_label(tr(cx, msg::settings::settings_page_refresh_provider_status))
                    .tab_index(0)
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .text_color(theme.text_muted)
                    .when(checking, |button| button.opacity(0.5))
                    .when(!checking, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.check_updates(cx);
                                this.load(cx);
                                cx.notify();
                            }))
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if !event.is_held
                                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                {
                                    this.check_updates(cx);
                                    this.load(cx);
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            }))
                    })
                    .child(
                        crate::icons::icon(crate::icons::REFRESH)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    ),
            );

        let scrollbar = popover::rail(self, "harnesses-page-scrollbar", &theme, cx);

        div()
            .id("harnesses-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("harnesses-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::settings_page_column(window)
                                .max_w(px(1040. + 2. * widgets::page_padding(window)))
                                .child(widgets::section_title_with_action(&theme, &tr(cx, msg::settings::settings_page_providers), None, "The coding agents on this computer, the accounts they sign in with, and the models they offer.", Some(div().flex().items_center().gap(px(6.)).child(check).into_any_element())))
                                .child(div().mt(px(20.)).flex().flex_col().gap(px(16.))
                                    .children(update_error)
                                    .child(body)
),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    #[gpui::test]
    fn expanded_agent_preferences_render_inside_the_agent_row(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            super::HarnessesPage::new(state, cx)
        });
        let descriptor = |id, name: &str| zuse_engine::registry::HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: false,
            steering_mode: zuse_protocol::SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: Some(true),
            signed_in: None,
        };
        window
            .update(cx, |page, _, cx| {
                page.harnesses = super::Loadable::Ready(vec![
                    descriptor(zuse_protocol::HarnessId::ClaudeCode, "Claude Code"),
                    descriptor(zuse_protocol::HarnessId::Codex, "Codex"),
                ]);
                page.select_provider(zuse_protocol::HarnessId::ClaudeCode, cx);
                assert_eq!(
                    page.expanded_harness,
                    Some(zuse_protocol::HarnessId::ClaudeCode)
                );
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        // Picking another provider replaces the detail pane; it never collapses.
        window
            .update(cx, |page, _, cx| {
                page.select_provider(zuse_protocol::HarnessId::Codex, cx);
                page.select_provider(zuse_protocol::HarnessId::Codex, cx);
                assert_eq!(page.expanded_harness, Some(zuse_protocol::HarnessId::Codex));
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        // Antigravity signs in through the very same Accounts tab.
        window
            .update(cx, |page, _, cx| {
                page.harnesses = super::Loadable::Ready(vec![descriptor(
                    zuse_protocol::HarnessId::Antigravity,
                    "Antigravity",
                )]);
                page.select_provider(zuse_protocol::HarnessId::Antigravity, cx);
                assert_eq!(
                    page.expanded_harness,
                    Some(zuse_protocol::HarnessId::Antigravity)
                );
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }
}

#[cfg(test)]
#[test]
fn install_visibility_and_hint_follow_target_capabilities() {
    for id in [
        HarnessId::Antigravity,
        HarnessId::Codex,
        HarnessId::Opencode,
        HarnessId::ClaudeCode,
        HarnessId::Cursor,
        HarnessId::Pi,
        HarnessId::Grok,
        HarnessId::Hermes,
        HarnessId::Devin,
        HarnessId::Muse,
        HarnessId::Mock,
    ] {
        for installed in [false, true] {
            for available in [false, true] {
                assert_eq!(
                    offers_install(id, installed, available),
                    id != HarnessId::Mock && !installed && available
                );
            }
        }
    }
    assert_eq!(cli_name(HarnessId::Antigravity), "Antigravity");
    assert_eq!(cli_name(HarnessId::Muse), "muse");
    assert_eq!(
        install_hint(HarnessId::Muse, false, false),
        "Install the muse CLI to enable. Install with `curl -fsSL https://dev.meta.ai/install.sh | bash`"
    );
    assert_eq!(
        install_hint(HarnessId::Antigravity, false, true),
        "Install Antigravity to enable"
    );
    assert!(
        install_hint(HarnessId::Antigravity, false, false).contains("ANTIGRAVITY_ACP_EXECUTABLE")
    );
}

#[cfg(test)]
#[test]
fn install_phase_copy_and_cancel_target_match_install() {
    assert_eq!(install_label("Claude Code"), "Installing Claude Code…");
    assert_eq!(install_label("Pi"), "Installing Pi…");
    for target in [None, Some("remote-device".to_string())] {
        let params = install_params(HarnessId::Pi, &target);
        assert_eq!(params["harness"], "pi");
        assert_eq!(
            params["targetDeviceId"],
            serde_json::to_value(&target).unwrap()
        );
    }
    assert_eq!(
        install_hint(HarnessId::Codex, false, false),
        "Install the codex CLI to enable. Install with `npm install -g @openai/codex`"
    );
    assert!(
        install_hint(HarnessId::Codex, true, false)
            .starts_with("codex CLI not installed — turn it off or install it.")
    );
}
