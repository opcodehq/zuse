//! The composer toolbar's usage indicator: one ring between the model pill
//! and the send key. Hovering or clicking it opens one glass card with the
//! context window ([`crate::context_usage`]) above the plan usage of the
//! session harness's live account: each rate-limit window with its meter and
//! reset, then the harness's other accounts. Clicking one switches to it, the
//! same `ActivateAgentAccount` Settings → Accounts runs. Both views share
//! [`AccountsSnapshotCache`], so a switch in either shows up in the other.
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use zuse_protocol::{AgentAccount, AgentAccountsSnapshot, HarnessId};
use zuse_rpc::methods;

use crate::popover;
use crate::settings::accounts::{
    self, AccountsSnapshotCache, UsageLevel, reports_usage, signs_in, usage_level,
};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Forced probes hit the provider; the engine throttles them too, but the
/// ring re-probes on every hover, so don't even ask more often than this.
const FORCE_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// Background re-probe while a composer is alive: usage moves as turns run.
const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How long a hover-opened card outlives the pointer leaving ring and card.
const HOVER_CLOSE_GRACE: Duration = Duration::from_millis(160);

/// The binding limit: the most-used window of the account. Pure.
pub fn used_fraction(account: &AgentAccount) -> Option<f32> {
    account
        .usage_windows
        .iter()
        .map(|window| window.used_fraction.clamp(0.0, 1.0))
        .reduce(f32::max)
}

/// The harness's live account, when it reports usage. Pure.
pub fn active_account(
    snapshot: &AgentAccountsSnapshot,
    harness: HarnessId,
) -> Option<&AgentAccount> {
    snapshot
        .accounts
        .iter()
        .find(|account| account.harness == harness && account.active)
}

/// Loads (and switches) the accounts of the composer's target device.
pub struct AccountUsage {
    state: Entity<AppState>,
    /// `None` = this device, as in [`AccountsSnapshotCache`].
    target: Option<String>,
    harness: Option<HarnessId>,
    /// Once any list has been asked for the current target.
    loaded: bool,
    last_forced: Option<Instant>,
    error: Option<SharedString>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    popup: popover::Popup<()>,
    /// Opened by a click: stays up until clicked again or away.
    pinned: bool,
    trigger_hovered: bool,
    card_hovered: bool,
    hover_close: Option<Task<()>>,
    /// Whether the toolbar shows the ring at all (not in side chats).
    visible: bool,
    _poll: Task<()>,
    _cache: Subscription,
    _state: Subscription,
}

impl AccountUsage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this
                    .update(cx, |usage, cx| {
                        if usage.harness.is_some() {
                            usage.load(true, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            _state: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            target: None,
            harness: None,
            loaded: false,
            last_forced: None,
            error: None,
            load_task: None,
            action_task: None,
            popup: popover::Popup::default(),
            pinned: false,
            trigger_hovered: false,
            card_hovered: false,
            hover_close: None,
            visible: true,
            _poll: poll,
            // Settings → Accounts writes the same cache.
            _cache: cx.observe_global::<AccountsSnapshotCache>(|_, cx| cx.notify()),
        }
    }

    /// Point at the session's harness and device; loads on first sight of a
    /// device. Cheap enough to call every render.
    pub fn track(
        &mut self,
        harness: Option<HarnessId>,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let harness = harness.filter(|h| signs_in(*h) && reports_usage(*h));
        self.harness = harness;
        if self.target != target {
            self.target = target;
            self.loaded = false;
            self.last_forced = None;
            self.error = None;
        }
        if harness.is_some() && !self.loaded {
            self.loaded = true;
            self.load(true, cx);
        }
    }

    /// Hide the ring (and close its card) where usage doesn't apply.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            if !visible {
                self.dismiss(cx);
            }
            cx.notify();
        }
    }

    fn snapshot<'a>(&self, cx: &'a gpui::App) -> Option<&'a AgentAccountsSnapshot> {
        cx.try_global::<AccountsSnapshotCache>()?
            .0
            .get(&self.target)
    }

    fn params(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let (Some(target), Some(object)) = (&self.target, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    /// Plain list first when nothing is cached (the engine's persisted usage
    /// paints at once), then the forced probe replaces it.
    fn load(&mut self, force_usage: bool, cx: &mut Context<Self>) {
        if force_usage {
            if self
                .last_forced
                .is_some_and(|at| at.elapsed() < FORCE_MIN_INTERVAL)
            {
                return;
            }
            self.last_forced = Some(Instant::now());
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let paint_first = force_usage && self.snapshot(cx).is_none();
        let plain = self.params(serde_json::json!({ "forceUsage": false }));
        let params = self.params(serde_json::json!({ "forceUsage": force_usage }));
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let fetch = |params: serde_json::Value| {
                let engine = engine.clone();
                async move {
                    engine
                        .client()
                        .call(methods::LIST_AGENT_ACCOUNTS, params)
                        .await
                        .ok()
                        .and_then(|value| {
                            serde_json::from_value::<AgentAccountsSnapshot>(value).ok()
                        })
                }
            };
            let store = |snapshot: AgentAccountsSnapshot, cx: &mut gpui::AsyncApp| {
                let key = key.clone();
                this.update(cx, |_, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(key, snapshot);
                })
                .ok();
            };
            if paint_first && let Some(snapshot) = fetch(plain).await {
                store(snapshot, cx);
            }
            if let Some(snapshot) = fetch(params).await {
                store(snapshot, cx);
            }
        }));
    }

    /// Switch optimistically, like Settings → Accounts: the rows flip at once
    /// and the engine's reply replaces them; a refusal restores the list.
    fn switch(&mut self, account: &AgentAccount, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.target.clone();
        let cache = &mut cx.default_global::<AccountsSnapshotCache>().0;
        let previous = cache.get(&key).cloned();
        if let Some(snapshot) = cache.get_mut(&key) {
            // Per-provider agents keep one live login per provider: only the
            // switched-to row's group changes.
            accounts::mark_switched(snapshot, account);
        }
        self.error = None;
        let params = self.params(serde_json::json!({
            "id": account.id,
            "accountId": account.id,
            "harness": account.harness,
        }));
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::ACTIVATE_AGENT_ACCOUNT, params)
                .await;
            this.update(cx, |usage, cx| {
                match result.map(serde_json::from_value::<AgentAccountsSnapshot>) {
                    Ok(Ok(snapshot)) => {
                        cx.default_global::<AccountsSnapshotCache>()
                            .0
                            .insert(key, snapshot);
                    }
                    Ok(Err(_)) => usage.load(false, cx),
                    Err(err) => {
                        if let Some(previous) = previous {
                            cx.default_global::<AccountsSnapshotCache>()
                                .0
                                .insert(key, previous);
                        }
                        usage.error = Some(err.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The ring's reading: the live account's most-used window, when the
    /// harness has a live account with usage to show.
    fn fraction(&self, cx: &gpui::App) -> Option<f32> {
        used_fraction(active_account(self.snapshot(cx)?, self.harness?)?)
    }

    /// Open the card from a hover: it closes again once the pointer leaves
    /// both the ring and the card.
    fn hover_open(&mut self, cx: &mut Context<Self>) {
        self.hover_close = None;
        if self.popup.is_open() {
            return;
        }
        // Opening the card is the moment someone cares: re-probe.
        self.load(true, cx);
        self.popup.open(());
        cx.notify();
    }

    /// A trigger click pins the card open (it stays up after the pointer
    /// leaves), or closes a pinned card.
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.popup.is_open() && self.pinned {
            self.dismiss(cx);
            return;
        }
        if !self.popup.is_open() {
            self.load(true, cx);
        }
        self.pinned = true;
        self.hover_close = None;
        self.popup.open(());
        cx.notify();
    }

    /// After a short grace, close a hover-opened card the pointer has left;
    /// the grace carries the pointer across the gap between ring and card.
    fn schedule_hover_close(&mut self, cx: &mut Context<Self>) {
        if self.pinned || !self.popup.is_open() {
            return;
        }
        self.hover_close = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_CLOSE_GRACE).await;
            this.update(cx, |usage, cx| {
                if !usage.pinned && !usage.trigger_hovered && !usage.card_hovered {
                    usage.dismiss(cx);
                }
            })
            .ok();
        }));
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.pinned = false;
        self.card_hovered = false;
        self.hover_close = None;
        if self.popup.begin_close() {
            popover::reap_popup(cx, |usage: &mut Self| &mut usage.popup);
        }
        cx.notify();
    }

    /// Review fixtures: this device's accounts for the session's harness —
    /// a live one with session and weekly windows, and one to switch to.
    #[cfg(feature = "browser-fixture")]
    pub fn fixture_seed(&mut self, cx: &mut Context<Self>) {
        let harness = self.harness.unwrap_or(HarnessId::ClaudeCode);
        let now = chrono::Utc::now();
        let account = |id: &str, email: &str, active: bool, windows: serde_json::Value| {
            serde_json::from_value::<AgentAccount>(serde_json::json!({
                "id": id, "harness": harness, "email": email, "planLabel": "Max",
                "active": active, "switchable": true, "usageWindows": windows,
            }))
            .unwrap()
        };
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account(
                    "fixture-work",
                    "alex@example.test",
                    true,
                    serde_json::json!([
                        {"label": "Session", "usedFraction": 0.38, "resetsAt": now + chrono::Duration::hours(3)},
                        {"label": "Weekly", "usedFraction": 0.12, "resetsAt": now + chrono::Duration::days(4)},
                    ]),
                ),
                account(
                    "fixture-home",
                    "sam@example.test",
                    false,
                    serde_json::json!([
                        {"label": "Session", "usedFraction": 0.05, "resetsAt": null},
                    ]),
                ),
            ],
            warnings: Vec::new(),
        };
        cx.default_global::<AccountsSnapshotCache>()
            .0
            .insert(self.target.clone(), snapshot);
        cx.notify();
    }

    /// Review fixtures: show the card pinned open.
    #[cfg(feature = "browser-fixture")]
    pub fn fixture_open(&mut self, cx: &mut Context<Self>) {
        self.pinned = true;
        self.popup.open(());
        cx.notify();
    }

    /// The plan-usage section: the live account's rate-limit windows, each
    /// with a meter and its reset, then the harness's other accounts to
    /// switch to. `None` when the harness has no accounts to show.
    fn plan_section(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let harness = self.harness?;
        let accounts: Vec<AgentAccount> = accounts::provider_accounts(self.snapshot(cx)?, harness)
            .into_iter()
            .cloned()
            .collect();
        if accounts.is_empty() && self.error.is_none() {
            return None;
        }
        let now = chrono::Utc::now();
        let active = accounts.iter().find(|account| account.active);
        let plan = active.and_then(|account| {
            account.plan_label.clone().or_else(|| {
                account
                    .email
                    .as_deref()
                    .map(zuse_protocol::presentation::masked_email)
            })
        });
        let windows = active.map_or(&[][..], |account| &account.usage_windows[..]);
        let mut section =
            crate::context_usage::section(theme).child(crate::context_usage::title_row(
                accounts::provider_name(harness),
                plan.unwrap_or_default(),
                theme,
            ));
        for window in windows {
            let used = window.used_fraction.clamp(0.0, 1.0);
            let reset = accounts::format_reset(window.resets_at, now)
                .map(|reset| reset.trim_start_matches("resets ").to_owned())
                .unwrap_or_else(|| "Unknown".into());
            section = section.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(crate::context_usage::value_row(
                        window.label.clone(),
                        format!("{:.0}% left", (1.0 - used) * 100.0),
                        theme.text,
                    ))
                    .child(crate::context_usage::stick_meter(
                        used as f64,
                        usage_level(used) != UsageLevel::Normal,
                        theme,
                    ))
                    .child(crate::context_usage::value_row(
                        "Resets",
                        reset,
                        theme.text_muted.opacity(0.7),
                    )),
            );
        }
        if windows.is_empty()
            && let Some(note) = active.and_then(|account| account.usage_error.clone())
        {
            section = section.child(
                div()
                    .text_color(theme.text_muted.opacity(0.7))
                    .child(SharedString::from(note)),
            );
        }
        let switchable: Vec<AgentAccount> = accounts
            .iter()
            .filter(|account| !account.active && account.switchable)
            .cloned()
            .collect();
        let switch_rows = (!switchable.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(popover::MENU_GAP))
                .px(px(popover::CARD_INSET))
                .pb(px(popover::CARD_INSET))
                .child(popover::menu_heading(theme, "Switch account"))
                .children(switchable.into_iter().enumerate().map(|(ix, account)| {
                    let email: SharedString = account
                        .email
                        .as_deref()
                        .map(zuse_protocol::presentation::masked_email)
                        .or_else(|| account.display_name.clone())
                        .unwrap_or_else(|| "Unknown account".into())
                        .into();
                    let meter = used_fraction(&account)
                        .map(|used| format!("{}% used", (used * 100.0).round() as u32));
                    let switch_to = account.clone();
                    popover::menu_row(theme, false, format!("account-usage-row-{ix}"))
                        .id(("account-usage-row", ix))
                        .when(!account.usage_windows.is_empty(), |row| {
                            row.tooltip(widgets::text_tooltip(accounts::usage_resets(
                                &account.usage_windows,
                                now,
                            )))
                        })
                        .on_click(cx.listener(move |usage, _, _, cx| {
                            usage.switch(&switch_to, cx);
                        }))
                        .child(div().flex_1().min_w_0().truncate().child(email))
                        .children(meter.map(|meter| {
                            div()
                                .flex_none()
                                .text_color(theme.text_muted)
                                .child(SharedString::from(meter))
                        }))
                }))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .child(section)
                .children(switch_rows)
                .children(self.error.clone().map(|error| {
                    div()
                        .px(px(14.0))
                        .pb(px(10.0))
                        .text_size(px(12.0))
                        .text_color(theme.danger)
                        .child(error)
                })),
        )
    }

    /// The usage card: the context window, then plan usage, in one glass
    /// surface.
    fn card(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = &Theme::of(cx).for_popup();
        let context =
            crate::context_usage::context_section(self.state.read(cx).context_usage, theme);
        let has_context = context.is_some();
        let plan = self.plan_section(theme, cx);
        popover::popover_card_flush(theme)
            .id("composer-usage-card")
            .w(px(crate::context_usage::CARD_WIDTH))
            .flex()
            .flex_col()
            .gap(px(0.0))
            .on_hover(cx.listener(|usage, hovered: &bool, _, cx| {
                usage.card_hovered = *hovered;
                if *hovered {
                    usage.hover_close = None;
                } else {
                    usage.schedule_hover_close(cx);
                }
            }))
            .children(context)
            .children(plan.map(|plan| {
                plan.when(has_context, |plan| {
                    plan.border_t_1().border_color(theme.border.opacity(0.6))
                })
            }))
    }
}

/// The composer toolbar's usage ring: context occupancy when the harness
/// reports a window, the live account's binding limit otherwise. Hover or
/// click opens one card with both.
impl Render for AccountUsage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let context = self.state.read(cx).context_usage;
        let account = self.fraction(cx);
        let mounted = self.popup.get().is_some();
        if !self.visible
            || (!crate::context_usage::has_window(context) && account.is_none() && !mounted)
        {
            return div().into_any_element();
        }
        let fraction = match crate::context_usage::reading(context) {
            Some(reading) => reading.fraction.map(|fraction| fraction as f32),
            None => account,
        };
        let high = crate::context_usage::is_high(context)
            || account.is_some_and(|used| usage_level(used) != UsageLevel::Normal);
        let ring = crate::context_usage::ring_button(fraction, high, mounted, &theme)
            .relative()
            // Its own gap to the send key, so an absent ring leaves none.
            .mr(px(crate::composer::ACTION_PRIMARY_GAP))
            .on_hover(cx.listener(|usage, hovered: &bool, _, cx| {
                usage.trigger_hovered = *hovered;
                if *hovered {
                    usage.hover_open(cx);
                } else {
                    usage.schedule_hover_close(cx);
                }
            }))
            .on_click(cx.listener(|usage, _, _, cx| usage.toggle(cx)));
        if !mounted {
            return ring.into_any_element();
        }
        let card = self
            .card(cx)
            .on_mouse_down_out(cx.listener(|usage, _, _, cx| {
                // A press on the ring itself is the ring's click to handle.
                if !usage.trigger_hovered {
                    usage.dismiss(cx);
                }
            }))
            .into_any_element();
        ring.child(popover::anchored_menu_above_end(
            "composer-usage-menu",
            card,
            self.popup.closing_since(),
        ))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_protocol::AgentUsageWindow;

    fn account(harness: HarnessId, active: bool, used: &[f32]) -> AgentAccount {
        serde_json::from_value(serde_json::json!({
            "id": format!("{harness:?}-{active}"),
            "harness": harness,
            "email": null,
            "planLabel": null,
            "active": active,
            "switchable": true,
            "usageWindows": used
                .iter()
                .map(|fraction| AgentUsageWindow {
                    label: "5h".into(),
                    used_fraction: *fraction,
                    resets_at: None,
                })
                .collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn ring_shows_the_most_used_window() {
        assert_eq!(used_fraction(&account(HarnessId::Codex, true, &[])), None);
        assert_eq!(
            used_fraction(&account(HarnessId::Codex, true, &[0.12, 0.64])),
            Some(0.64)
        );
        assert_eq!(
            used_fraction(&account(HarnessId::Codex, true, &[1.4])),
            Some(1.0)
        );
    }

    #[test]
    fn active_account_is_scoped_to_the_harness() {
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account(HarnessId::ClaudeCode, true, &[0.3]),
                account(HarnessId::Codex, false, &[0.1]),
                account(HarnessId::Codex, true, &[0.2]),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            active_account(&snapshot, HarnessId::Codex).map(|a| a.id.as_str()),
            Some("Codex-true")
        );
        assert!(active_account(&snapshot, HarnessId::Cursor).is_none());
    }
}
