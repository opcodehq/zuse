//! Desktop Usage and Billing pages. Usage maps compute spend to the computers
//! (and so the chats) that used it; Billing shows the plan and money left.
use super::*;
use crate::cloud_settings::style::icon_label_button;
use std::collections::BTreeMap;

const DAY_MS: i64 = 86_400_000;

/// How the host names a computer: the chat it ran and where it worked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Tasks,
    Builds,
    Storage,
}
impl Kind {
    fn of(item: &BillingUsageItem) -> Self {
        match (item.resource_kind.as_str(), item.usage_kind.as_deref()) {
            (_, Some("snapshot-storage")) => Kind::Storage,
            ("workspace", _) => Kind::Tasks,
            ("build", _) => Kind::Builds,
            _ => Kind::Storage,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Tasks => "Cloud computers",
            Kind::Builds => "Image builds",
            Kind::Storage => "Image storage",
        }
    }
    fn color(self) -> Hsla {
        match self {
            Kind::Tasks => palette::tone(
                hsla(83. / 360., 0.72, 0.46, 1.),
                hsla(83. / 360., 0.74, 0.40, 1.),
            ),
            Kind::Builds => palette::tone(
                hsla(190. / 360., 0.55, 0.50, 1.),
                hsla(190. / 360., 0.60, 0.38, 1.),
            ),
            Kind::Storage => palette::rgb(MUTED).into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub resource_id: String,
    pub cost_micros: i64,
    pub busy_ms: i64,
    pub vcpu: i64,
    pub memory_mib: i64,
    pub provisional: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Breakdown {
    pub total_micros: i64,
    pub by_kind: BTreeMap<Kind, Vec<Line>>,
    /// Day index from the period start → cost per kind.
    pub days: Vec<[i64; 3]>,
}

/// Groups usage entries per resource, most expensive first, and spreads cost
/// over the days of the period. Pure, so it is tested without a UI.
pub fn breakdown(items: &[BillingUsageItem], start: i64, end: i64) -> Breakdown {
    let span = ((end - start).max(DAY_MS) + DAY_MS - 1) / DAY_MS;
    let mut out = Breakdown {
        days: vec![[0; 3]; span.clamp(1, 62) as usize],
        ..Default::default()
    };
    let mut lines: BTreeMap<(Kind, String), Line> = BTreeMap::new();
    for item in items {
        let kind = Kind::of(item);
        let cost = item.provider_cost_micros.max(0);
        out.total_micros += cost;
        let line = lines
            .entry((kind, item.resource_id.clone()))
            .or_insert_with(|| Line {
                resource_id: item.resource_id.clone(),
                ..Default::default()
            });
        line.cost_micros += cost;
        line.busy_ms += (item.ended_at - item.started_at).max(0);
        line.vcpu = line.vcpu.max(item.vcpu_count);
        line.memory_mib = line.memory_mib.max(item.memory_mib);
        line.provisional |= item.status == "provisional";
        let day = ((item.started_at - start) / DAY_MS).clamp(0, out.days.len() as i64 - 1);
        out.days[day as usize][kind as usize] += cost;
    }
    for ((kind, _), line) in lines {
        out.by_kind.entry(kind).or_default().push(line);
    }
    for lines in out.by_kind.values_mut() {
        lines.sort_by(|a, b| b.cost_micros.cmp(&a.cost_micros));
    }
    out
}

fn hours(ms: i64) -> String {
    let h = ms as f64 / 3_600_000.;
    if h < 0.1 && ms > 0 {
        format!("{}m", (ms / 60_000).max(1))
    } else {
        format!("{h:.1}h")
    }
}
fn share(part: i64, total: i64) -> String {
    if total <= 0 {
        "0%".into()
    } else {
        format!("{:.1}%", part as f64 * 100. / total as f64)
    }
}
fn date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%b %-d").to_string())
        .unwrap_or_default()
}
fn long_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%b %-d, %Y")
                .to_string()
        })
        .unwrap_or_default()
}
fn caption(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .line_height(px(15.))
        .text_color(palette::rgb(MUTED))
        .child(text.into())
}
fn amount(text: impl Into<SharedString>, size: f32) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(size * 1.25))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette::rgb(TEXT))
        .child(text.into())
}
fn meter(fraction: f32, warning: bool) -> Div {
    let fill = if warning {
        hsla(21.6 / 360., 0.982, 0.567, 1.)
    } else {
        Kind::Tasks.color()
    };
    div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(s::frame_bg())
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(fill)
                .w(relative(fraction.clamp(0., 1.))),
        )
}
fn card_body() -> Div {
    div().flex().flex_col().gap(px(10.)).p(px(12.))
}
fn alert(error: &str) -> Div {
    note(error, true)
}

/// The period header both pages share: dates, days left and money left.
fn period<T: 'static>(
    summary: &BillingSummary,
    link: (&'static str, &'static str, Action),
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let now = chrono::Utc::now().timestamp_millis();
    let left = ((summary.period_end - now).max(0) + DAY_MS - 1) / DAY_MS;
    let (id, label, action) = link;
    s::frame().child(
        s::card().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .px(px(12.))
                .py(px(10.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(s::title_text(format!(
                            "{} – {}",
                            date(summary.period_start),
                            date(summary.period_end)
                        )))
                        .child(caption(format!(
                            "{left} day{} left in cycle",
                            if left == 1 { "" } else { "s" }
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(4.))
                                .child(amount(usd(summary.included_remaining_micros), 13.))
                                .child(caption("included left")),
                        )
                        .child(button(id, label, false, false, false).on_click(
                            cx.listener(move |this, _, window, cx| act(this, action, window, cx)),
                        )),
                ),
        ),
    )
}

fn chart(breakdown: &Breakdown, start: i64) -> Div {
    let now = chrono::Utc::now().timestamp_millis();
    let shown = (((now - start) / DAY_MS) + 1).clamp(1, breakdown.days.len() as i64) as usize;
    let days = &breakdown.days[..shown];
    let max = days
        .iter()
        .map(|d| d.iter().sum::<i64>())
        .max()
        .unwrap_or(0)
        .max(1);
    const HEIGHT: f32 = 120.;
    let mut bars = div()
        .flex()
        .items_end()
        .gap(px(3.))
        .h(px(HEIGHT))
        .w_full()
        .border_b_1()
        .border_color(palette::rgb(BORDER));
    for (i, day) in days.iter().enumerate() {
        let total: i64 = day.iter().sum();
        let mut bar = div()
            .id(("usage-day", i))
            .role(Role::Image)
            .aria_label(SharedString::from(format!(
                "{}: {}",
                date(start + i as i64 * DAY_MS),
                usd(total)
            )))
            .flex_1()
            .min_w(px(2.))
            .max_w(px(28.))
            .flex()
            .flex_col_reverse()
            .h(px(HEIGHT * total as f32 / max as f32))
            .rounded_t(px(2.))
            .overflow_hidden();
        if total == 0 {
            bar = bar.h(px(1.)).bg(palette::rgb(BORDER));
        }
        for kind in [Kind::Tasks, Kind::Builds, Kind::Storage] {
            let part = day[kind as usize];
            if part > 0 {
                bar = bar.child(
                    div()
                        .w_full()
                        .h(relative(part as f32 / total as f32))
                        .bg(kind.color()),
                );
            }
        }
        bars = bars.child(bar);
    }
    let mut axis = div().flex().justify_between().mt(px(6.));
    for i in [0, shown / 2, shown.saturating_sub(1)] {
        axis = axis.child(caption(date(start + i as i64 * DAY_MS)));
    }
    if shown == 1 {
        axis = div().flex().mt(px(6.)).child(caption(date(start)));
    }
    let mut legend = div().flex().gap(px(14.)).mt(px(10.));
    for kind in [Kind::Tasks, Kind::Builds, Kind::Storage] {
        legend = legend.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().size(px(8.)).rounded(px(2.)).bg(kind.color()))
                .child(caption(kind.label())),
        );
    }
    div().child(bars).child(axis).child(legend)
}

fn table_header() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(8.))
        .text_size(px(11.))
        .text_color(palette::rgb(MUTED))
        .child(div().flex_1().child("Item"))
        .child(div().w(px(64.)).text_right().child("Time"))
        .child(div().w(px(72.)).text_right().child("Spend"))
        .child(div().w(px(56.)).text_right().child("Share"))
}
fn table_row(
    title: String,
    detail: Option<String>,
    time: String,
    spend: String,
    share: String,
    heading: bool,
) -> Div {
    let mut copy = div().flex_1().min_w_0().child(
        div()
            .truncate()
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(palette::rgb(TEXT))
            .when(heading, |v| v.font_weight(FontWeight::MEDIUM))
            .child(title),
    );
    if let Some(detail) = detail {
        copy = copy.child(caption(detail).truncate());
    }
    let cell = |w: f32, text: String| {
        div()
            .w(px(w))
            .flex_none()
            .text_right()
            .text_size(px(12.))
            .text_color(palette::rgb(if heading { TEXT } else { MUTED }))
            .when(heading, |v| v.font_weight(FontWeight::MEDIUM))
            .child(text)
    };
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(if heading { 7. } else { 8. }))
        .border_t_1()
        .border_color(palette::rgb(BORDER))
        .when(heading, |v| v.bg(s::frame_bg().opacity(0.35)))
        .child(copy)
        .child(cell(64., time))
        .child(cell(72., spend))
        .child(cell(56., share))
}

/// Usage → Cloud compute: spend over the period and which computers used it.
pub fn compute<T: 'static>(
    data: &Data<'_>,
    machines: &BTreeMap<String, Machine>,
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let mut root = div().flex().flex_col().gap(px(16.));
    if let Some(error) = data.error {
        root = root.child(alert(error));
    }
    if data.subscribed == Some(false) {
        return root.child(group(
            "Cloud compute",
            None,
            None,
            div().child(setting_row(
                true,
                "No Cloud plan yet",
                "Cloud computers are billed by Zuse. Subscribe to start cloud tasks; usage appears here as soon as a computer runs.",
                Some(
                    button("usage-open-billing", "See plans", true, false, false)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            act(this, Action::OpenBilling, window, cx)
                        }))
                        .into_any_element(),
                ),
            )),
        ));
    }
    let Some(summary) = data.summary else {
        return root.child(group(
            "Cloud compute",
            None,
            None,
            div().child(setting_row(
                true,
                if data.pending.loading {
                    "Loading usage…"
                } else {
                    "Usage is unavailable"
                },
                "Compute and storage for this billing period.",
                (!data.pending.loading).then(|| {
                    button("retry-cloud-usage", "Retry", false, false, false)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            act(this, Action::Refresh, window, cx)
                        }))
                        .into_any_element()
                }),
            )),
        ));
    };
    let breakdown = breakdown(data.usage, summary.period_start, summary.period_end);
    root = root.child(period(
        summary,
        (
            "usage-manage-billing",
            "Manage billing",
            Action::OpenBilling,
        ),
        cx,
        act,
    ));
    let mut spend = card_body().child(
        div()
            .flex()
            .items_baseline()
            .gap(px(8.))
            .child(amount(usd(breakdown.total_micros), 20.))
            .child(caption(format!(
                "compute and storage, {} – today{}",
                date(summary.period_start),
                if summary.usage_provisional {
                    " · still settling"
                } else {
                    ""
                }
            ))),
    );
    spend = spend.child(chart(&breakdown, summary.period_start));
    root = root.child(group("Spend", None, None, spend));
    let mut table = div().flex().flex_col().child(table_header());
    if breakdown.by_kind.is_empty() {
        table = table.child(setting_row(
            false,
            "No usage yet this period",
            "Start a cloud chat. Each computer it runs on shows up here with its time and cost.",
            None,
        ));
    }
    for (kind, lines) in &breakdown.by_kind {
        let cost: i64 = lines.iter().map(|l| l.cost_micros).sum();
        let busy: i64 = lines.iter().map(|l| l.busy_ms).sum();
        table = table.child(table_row(
            kind.label().into(),
            None,
            if *kind == Kind::Storage {
                "—".into()
            } else {
                hours(busy)
            },
            usd(cost),
            share(cost, breakdown.total_micros),
            true,
        ));
        let limit = if data.history_open { usize::MAX } else { 8 };
        for line in lines.iter().take(limit) {
            let machine = machines.get(&line.resource_id);
            let size = (line.vcpu > 0)
                .then(|| format!("{} vCPU · {} GB", line.vcpu, (line.memory_mib + 512) / 1024));
            let (title, detail) = match (kind, machine) {
                (Kind::Tasks, Some(m)) => (
                    m.title.clone(),
                    Some(
                        [Some(m.detail.clone()).filter(|d| !d.is_empty()), size]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · "),
                    ),
                ),
                (Kind::Tasks, None) => ("Cloud computer".into(), size),
                (Kind::Builds, _) => ("Machine image build".into(), size),
                (Kind::Storage, _) => ("Saved machine image".into(), None),
            };
            table = table.child(table_row(
                title + if line.provisional { " · running" } else { "" },
                detail.filter(|d| !d.is_empty()),
                if *kind == Kind::Storage {
                    "—".into()
                } else {
                    hours(line.busy_ms)
                },
                usd(line.cost_micros),
                share(line.cost_micros, breakdown.total_micros),
                false,
            ));
        }
    }
    let hidden = breakdown.by_kind.values().any(|l| l.len() > 8);
    if hidden || data.more {
        table = table.child(
            div()
                .px(px(12.))
                .py(px(10.))
                .border_t_1()
                .border_color(palette::rgb(BORDER))
                .child(
                    button(
                        "toggle-cloud-usage-history",
                        if data.history_open {
                            "Show fewer"
                        } else {
                            "Show every computer"
                        },
                        false,
                        false,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        act(this, Action::ToggleUsageHistory, window, cx)
                    })),
                ),
        );
    }
    root.child(group("Breakdown", None, None, table)).child(caption(
        "Costs are Zuse's provider cost for each computer, before your plan's included allowance. Model usage on your own subscriptions is under Models.",
    ))
}

/// Billing: the plan, money left, the cap, credits and who bills what.
pub fn billing<T: 'static>(
    data: &Data<'_>,
    cap: AnyElement,
    focus: &[FocusHandle; 4],
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let mut root = div().flex().flex_col().gap(px(16.));
    if let Some(error) = data.error {
        root = root.child(alert(error));
    }
    let busy = data.pending.subscription;
    let (status, warning, detail) = match (data.subscribed, data.summary) {
        (Some(true), Some(summary)) => {
            let (label, warning) = match summary.status {
                BillingStatus::Active | BillingStatus::Manual => ("Active", false),
                BillingStatus::Grace => ("Payment due", true),
                BillingStatus::BillingHold => ("Paused", true),
                BillingStatus::Ended => ("Ended", true),
            };
            (
                label,
                warning,
                format!(
                    "{}/month · renews {}",
                    usd(summary.base_price_micros),
                    long_date(summary.period_end)
                ),
            )
        }
        (Some(true), None) => ("Active", false, "Subscribed".to_owned()),
        (Some(false), _) => (
            "Not subscribed",
            false,
            "$40/month, including $35 of cloud compute and image storage.".to_owned(),
        ),
        (None, _) => (
            if data.pending.loading {
                "Loading…"
            } else {
                "Unavailable"
            },
            false,
            "Checking your plan.".to_owned(),
        ),
    };
    let plan_action = match data.subscribed {
        Some(true) => Some(button(
            "manage-cloud-plan",
            "Manage plan",
            false,
            busy,
            busy,
        )),
        Some(false) => Some(crate::styling::dither_action(
            "subscribe-cloud-plan",
            "Subscribe",
            !busy,
            true,
        )),
        None if !data.pending.loading => Some(button(
            "retry-cloud-subscription",
            "Retry",
            false,
            false,
            false,
        )),
        None => None,
    }
    .map(|b| {
        let action = match data.subscribed {
            Some(true) => Action::ManageSubscription,
            Some(false) => Action::Subscription,
            None => Action::Refresh,
        };
        b.on_click(cx.listener(move |this, _, window, cx| {
            if !busy {
                act(this, action, window, cx)
            }
        }))
        .into_any_element()
    });
    let mut plan = div().flex().flex_col().child(
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(s::title_text("Zuse Cloud"))
                            .child(badge(status, warning, data.subscribed != Some(true))),
                    )
                    .child(div().mt(px(2.)).child(s::description_text(detail))),
            )
            .when_some(plan_action, |v, a| v.child(a)),
    );
    if let (Some(true), Some(summary)) = (data.subscribed, data.summary) {
        let included = summary.included_provider_cost_micros.max(1);
        let used = summary.included_used_micros;
        let fraction = used as f32 / included as f32;
        plan = plan.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(12.))
                .py(px(12.))
                .border_t_1()
                .border_color(palette::rgb(BORDER))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .child(s::title_text("Included compute"))
                        .child(amount(usd(summary.included_remaining_micros), 18.)),
                )
                .child(meter(fraction, fraction >= 0.9))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .child(caption(format!(
                            "Used {} of {}",
                            usd(used),
                            usd(summary.included_provider_cost_micros)
                        )))
                        .child(
                            button("billing-open-usage", "See usage", false, false, false)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act(this, Action::OpenUsage, window, cx)
                                })),
                        ),
                ),
        );
        plan = plan.child(setting_row(
            false,
            "This invoice so far",
            format!(
                "{} plan + {} over the allowance (with {}% markup). Before tax.",
                usd(summary.base_price_micros),
                usd(summary.overage_charge_micros),
                summary.markup_basis_points as f64 / 100.
            ),
            Some(amount(usd(summary.current_invoice_estimate_micros), 13.).into_any_element()),
        ));
        let disabled = data.pending.loading || data.pending.cap;
        plan = plan.child(setting_row(
            false,
            "Spending cap",
            "Monthly limit on usage beyond the allowance. Builds and cloud computers pause when it's reached.",
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .w(px(80.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .px(px(9.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(palette::rgb(BORDER))
                            .child(cap),
                    )
                    .child(
                        button("save-cloud-cap", "Save", true, disabled, data.pending.cap)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !disabled {
                                    act(this, Action::SaveCap, window, cx)
                                }
                            })),
                    )
                    .into_any_element(),
            ),
        ));
    }
    root = root.child(group("Plan", None, None, plan));
    root = root.child(group(
        "Credits",
        None,
        None,
        credits_body(data, focus, cx, act),
    ));
    let sources = div()
        .flex()
        .flex_col()
        .child(setting_row(
            true,
            "Billed by Zuse",
            "Cloud computers, image builds and image storage. Paid from your plan's allowance, then credits, then your invoice.",
            None,
        ))
        .child(setting_row(
            false,
            "Billed by your agent providers",
            "Codex, Claude and Grok run on the subscription you sign in with, locally and in Cloud. Zuse never charges for their tokens.",
            Some(
                button("billing-open-agents", "Agent accounts", false, false, false)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        act(this, Action::OpenAgents, window, cx)
                    }))
                    .into_any_element(),
            ),
        ));
    root = root.child(group("Who bills what", None, None, sources));
    let disabled = data.subscribed != Some(true) || busy;
    root.child(group(
        "Payment",
        None,
        None,
        div().child(setting_row(
            true,
            "Payment method and invoices",
            "Change your card, download invoices or cancel in the billing portal.",
            Some(
                icon_label_button(
                    "manage-cloud-subscription",
                    "arrow-up-right",
                    "Open portal",
                    false,
                    disabled,
                    busy,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !disabled {
                        act(this, Action::ManageSubscription, window, cx)
                    }
                }))
                .into_any_element(),
            ),
        )),
    ))
}

#[cfg(test)]
mod tests {
    use super::{DAY_MS, Kind, breakdown};
    use zuse_protocol::cloud_billing::BillingUsageItem;
    fn item(kind: &str, id: &str, start: i64, minutes: i64, cost: i64) -> BillingUsageItem {
        BillingUsageItem {
            entry_id: format!("{id}-{start}"),
            resource_kind: kind.into(),
            usage_kind: None,
            resource_id: id.into(),
            provider: "boxd".into(),
            provider_execution_id: None,
            started_at: start,
            ended_at: start + minutes * 60_000,
            vcpu_count: 2,
            memory_mib: 4096,
            provider_cost_micros: cost,
            status: "confirmed".into(),
        }
    }
    #[test]
    fn usage_groups_by_computer_and_spreads_cost_over_days() {
        let start = 1_700_000_000_000;
        let items = [
            item("workspace", "w1", start, 30, 100),
            item("workspace", "w1", start + DAY_MS, 30, 50),
            item("workspace", "w2", start + DAY_MS, 60, 400),
            item("build", "b1", start, 10, 20),
        ];
        let b = breakdown(&items, start, start + 30 * DAY_MS);
        assert_eq!(b.total_micros, 570);
        assert_eq!(b.days.len(), 30);
        assert_eq!(b.days[0], [100, 20, 0]);
        assert_eq!(b.days[1], [450, 0, 0]);
        let tasks = &b.by_kind[&Kind::Tasks];
        assert_eq!(tasks[0].resource_id, "w2");
        assert_eq!(tasks[1].cost_micros, 150);
        assert_eq!(tasks[1].busy_ms, 60 * 60_000);
        assert_eq!(b.by_kind[&Kind::Builds].len(), 1);
    }
}
