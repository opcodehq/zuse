//! Desktop Usage and Billing pages. Usage maps machine minutes to the days,
//! chats and computers that used them; Billing shows the plan, what is left,
//! the cap and the plans to change to. Machine minutes are the headline,
//! hours a hint, dollars only on prices, pay as you go and invoices.
use super::*;
use crate::cloud_settings::style::icon_label_button;
use crate::dither_columns::{Column, columns};
use crate::tooltip::TooltipExt as _;
use std::collections::BTreeMap;
use zuse_model::cloud_billing::{grouped, machine_size};
use zuse_protocol::cloud_billing::PlanUsage;
use zuse_protocol::cloud_billing::plans::{self as plan_math, DEFAULT_RATE};

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
            Kind::Tasks => "Chats",
            Kind::Builds => "Machine image builds",
            Kind::Storage => "Saved images",
        }
    }
    fn legend(self) -> &'static str {
        match self {
            Kind::Tasks => "Cloud computers",
            Kind::Builds => "Image builds",
            Kind::Storage => "Storage",
        }
    }
    fn color(self) -> Hsla {
        match self {
            Kind::Tasks => palette::accent_or(palette::tone(
                hsla(83. / 360., 0.72, 0.46, 1.),
                hsla(83. / 360., 0.74, 0.40, 1.),
            )),
            Kind::Builds => palette::tone(
                hsla(190. / 360., 0.55, 0.55, 1.),
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
    /// Time awake.
    pub busy_ms: i64,
    pub vcpu: i64,
    pub memory_mib: i64,
    pub provisional: bool,
    /// Machine minutes used, and the fastest rate the computer ran at.
    pub minutes: i64,
    pub rate: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Breakdown {
    pub total_micros: i64,
    pub total_minutes: i64,
    pub by_kind: BTreeMap<Kind, Vec<Line>>,
    /// Day index from the period start → machine minutes per kind, or cost
    /// per kind for periods billed before machine minutes.
    pub days: Vec<[i64; 3]>,
}

fn entry_rate(item: &BillingUsageItem) -> u32 {
    item.machine_rate.unwrap_or_else(|| {
        plan_math::machine_rate(item.memory_mib.clamp(0, u32::MAX as i64) as u32)
    })
}
fn entry_awake(item: &BillingUsageItem) -> i64 {
    item.awake_ms
        .unwrap_or(item.ended_at - item.started_at)
        .max(0)
}
/// Machine minutes an entry used: the server's figure, else awake time
/// times the machine rate. Storage uses none.
fn entry_minutes(item: &BillingUsageItem) -> i64 {
    if Kind::of(item) == Kind::Storage {
        return 0;
    }
    item.machine_minutes
        .unwrap_or_else(|| plan_math::machine_minutes(entry_awake(item), entry_rate(item)))
}

/// Groups usage entries per resource, largest first, and spreads machine
/// minutes (or cost, without a plan) over the days of the period. Pure, so
/// it is tested without a UI.
pub fn breakdown(items: &[BillingUsageItem], start: i64, end: i64, by_minutes: bool) -> Breakdown {
    let span = ((end - start).max(DAY_MS) + DAY_MS - 1) / DAY_MS;
    let mut out = Breakdown {
        days: vec![[0; 3]; span.clamp(1, 62) as usize],
        ..Default::default()
    };
    let mut lines: BTreeMap<(Kind, String), Line> = BTreeMap::new();
    for item in items {
        let kind = Kind::of(item);
        let cost = item.provider_cost_micros.max(0);
        let minutes = entry_minutes(item);
        out.total_micros += cost;
        out.total_minutes += minutes;
        let line = lines
            .entry((kind, item.resource_id.clone()))
            .or_insert_with(|| Line {
                resource_id: item.resource_id.clone(),
                ..Default::default()
            });
        line.cost_micros += cost;
        line.minutes += minutes;
        if kind != Kind::Storage {
            line.rate = line.rate.max(entry_rate(item));
            line.busy_ms += entry_awake(item);
        }
        line.vcpu = line.vcpu.max(item.vcpu_count);
        line.memory_mib = line.memory_mib.max(item.memory_mib);
        line.provisional |= item.status == "provisional";
        let day = ((item.started_at - start) / DAY_MS).clamp(0, out.days.len() as i64 - 1);
        out.days[day as usize][kind as usize] += if by_minutes { minutes } else { cost };
    }
    for ((kind, _), line) in lines {
        out.by_kind.entry(kind).or_default().push(line);
    }
    for lines in out.by_kind.values_mut() {
        if by_minutes {
            lines.sort_by(|a, b| b.minutes.cmp(&a.minutes).then(b.busy_ms.cmp(&a.busy_ms)));
        } else {
            lines.sort_by(|a, b| b.cost_micros.cmp(&a.cost_micros));
        }
    }
    out
}

/// Awake time as hours: "5.2 h", "<0.1 h".
fn awake(ms: i64) -> String {
    let tenths = ms.max(0) / 360_000;
    if ms <= 0 {
        "—".into()
    } else if tenths == 0 {
        "<0.1 h".into()
    } else if tenths >= 100 {
        format!("{} h", tenths / 10)
    } else {
        format!("{}.{} h", tenths / 10, tenths % 10)
    }
}
fn share(part: i64, total: i64) -> f32 {
    if total <= 0 {
        0.
    } else {
        (part as f64 / total as f64) as f32
    }
}
fn percent(fraction: f32) -> String {
    if fraction <= 0. {
        "0%".into()
    } else if fraction < 0.01 {
        "<1%".into()
    } else {
        format!("{:.0}%", fraction * 100.)
    }
}
use plans::date;
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
fn alert(error: &str) -> Div {
    s::frame().child(s::card().child(note(error, true)))
}
fn days_left(end: i64) -> i64 {
    let now = chrono::Utc::now().timestamp_millis();
    ((end - now).max(0) + DAY_MS - 1) / DAY_MS
}
/// A page section: the settings frame with a title, optional description
/// and action, around a card.
fn section(
    title: &'static str,
    description: impl Into<SharedString>,
    action: Option<AnyElement>,
    body: impl IntoElement,
) -> Div {
    s::section(s::header_with_action(title, description, action), body)
}
/// One figure of the usage header: label, big value and a hint.
fn stat(label: &'static str, value: String, unit: &'static str, hint: String) -> Div {
    div()
        .flex_1()
        .min_w(px(140.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(caption(label))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .child(amount(value, 20.))
                .when(!unit.is_empty(), |v| v.child(caption(unit))),
        )
        .child(caption(hint))
}

/// Dithered columns of machine minutes per day, with a scale and dates.
fn chart(breakdown: &Breakdown, start: i64, by_minutes: bool, rate: u32) -> Div {
    let now = chrono::Utc::now().timestamp_millis();
    let shown = (((now - start) / DAY_MS) + 1).clamp(1, breakdown.days.len() as i64) as usize;
    // Show at least two weeks so a new cycle doesn't draw a few fat columns.
    let slots = shown.max(14).min(breakdown.days.len());
    let max = breakdown.days[..shown]
        .iter()
        .map(|d| d.iter().sum::<i64>())
        .max()
        .unwrap_or(0)
        .max(1);
    // A round scale top so the label reads cleanly.
    let step = [
        10, 20, 50, 100, 200, 250, 500, 1_000, 2_000, 2_500, 5_000, 10_000,
    ]
    .into_iter()
    .find(|step| max <= step * 4)
    .unwrap_or(25_000);
    let top = (max + step - 1) / step * step;
    let value = |v: i64| {
        if by_minutes { grouped(v) } else { usd(v) }
    };
    let data = (0..slots)
        .map(|i| {
            let day = breakdown.days.get(i).copied().unwrap_or_default();
            let total: i64 = day.iter().sum();
            let when = date(start + i as i64 * DAY_MS);
            let label = if i >= shown {
                format!("{when} · upcoming")
            } else if by_minutes {
                format!(
                    "{when} · {} ({} on {})",
                    machine_minutes(total),
                    about_hours(total, rate),
                    machine_size(rate, rate * plan_math::UNIT_MEMORY_MIB)
                )
            } else {
                format!("{when} · {}", usd(total))
            };
            Column {
                segments: if i >= shown {
                    vec![]
                } else {
                    [Kind::Tasks, Kind::Builds, Kind::Storage]
                        .into_iter()
                        .map(|k| (day[k as usize] as f64, k.color()))
                        .collect()
                },
                label: label.into(),
            }
        })
        .collect();
    const HEIGHT: f32 = 140.;
    let grid = |at: f32| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(at))
            .h(px(1.))
            .bg(palette::rgb(BORDER).opacity(0.7))
    };
    let plot = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(HEIGHT))
        .child(grid(0.))
        .child(grid(HEIGHT / 2.))
        .child(columns(
            "usage-day",
            data,
            top as f64,
            HEIGHT,
            palette::rgb(MUTED).opacity(0.35).into(),
            s::frame_bg().opacity(0.6),
        ));
    let scale = div()
        .w(px(44.))
        .flex_none()
        .h(px(HEIGHT))
        .flex()
        .flex_col()
        .justify_between()
        .items_end()
        .child(caption(value(top)).mt(px(-7.)))
        .child(caption(value(top / 2)))
        .child(caption("0").mb(px(-7.)));
    let mut axis = div().flex().justify_between().mt(px(8.)).ml(px(54.));
    for i in [0, slots / 2, slots.saturating_sub(1)] {
        axis = axis.child(caption(date(start + i as i64 * DAY_MS)));
    }
    let mut legend = div().flex().flex_wrap().gap(px(16.));
    let kinds: &[Kind] = if by_minutes {
        &[Kind::Tasks, Kind::Builds]
    } else {
        &[Kind::Tasks, Kind::Builds, Kind::Storage]
    };
    for kind in kinds {
        legend = legend.child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .relative()
                        .size(px(8.))
                        .child(crate::dither::fill_color(kind.color(), 0.)),
                )
                .child(caption(kind.legend())),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(14.))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(scale)
                .child(plot),
        )
        .child(axis.mt(px(0.)))
        .child(legend)
}

fn table_header(spend: &'static str) -> Div {
    let cell = |w: f32, text: &'static str| div().w(px(w)).flex_none().text_right().child(text);
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(8.))
        .text_size(px(11.))
        .line_height(px(15.))
        .text_color(palette::rgb(MUTED))
        .child(div().flex_1().child("Chat and computer"))
        .child(cell(64., "Awake"))
        .child(cell(156., spend))
        .child(div().w(px(96.)).flex_none().child("Share"))
}
struct Row {
    id: String,
    title: String,
    detail: Option<String>,
    time: String,
    spend: String,
    hint: Option<String>,
    share: Option<f32>,
    heading: bool,
}
fn table_row(row: Row) -> Stateful<Div> {
    let mut copy = div().flex_1().min_w_0().child(
        div()
            .truncate()
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(palette::rgb(TEXT))
            .when(row.heading, |v| v.font_weight(FontWeight::MEDIUM))
            .child(row.title),
    );
    if let Some(detail) = row.detail {
        copy = copy.child(caption(detail).truncate());
    }
    let cell = |w: f32, text: String| {
        div()
            .w(px(w))
            .flex_none()
            .text_right()
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(palette::rgb(if row.heading { TEXT } else { MUTED }))
            .when(row.heading, |v| v.font_weight(FontWeight::MEDIUM))
            .child(text)
    };
    let mut spend = div()
        .id(SharedString::from(format!("{}-minutes", row.id)))
        .w(px(156.))
        .flex_none()
        .flex()
        .justify_end()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(palette::rgb(TEXT))
        .when(row.heading, |v| v.font_weight(FontWeight::MEDIUM))
        .child(row.spend);
    if let Some(hint) = row.hint {
        spend = spend.tip(hint);
    }
    let share = div()
        .w(px(96.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .when_some(row.share, |v, fraction| {
            v.child(
                div()
                    .relative()
                    .w(px(56.))
                    .h(px(6.))
                    .child(crate::dither::meter(
                        fraction,
                        Kind::Tasks.color(),
                        palette::rgb(MUTED).opacity(0.3).into(),
                    )),
            )
            .child(caption(percent(fraction)))
        });
    div()
        .id(SharedString::from(row.id))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(if row.heading { 8. } else { 9. }))
        .border_t_1()
        .border_color(palette::rgb(BORDER))
        .when(row.heading, |v| v.bg(s::frame_bg().opacity(0.4)))
        .when(!row.heading, |v| {
            v.hover(|v| v.bg(s::frame_bg().opacity(0.3)))
        })
        .child(copy)
        .child(cell(64., row.time))
        .child(spend)
        .child(share)
}

/// Usage → Cloud compute: machine minutes over the cycle, by day and by
/// the chats (and their computers) that used them.
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
        return root.child(section(
            "Cloud compute",
            "",
            None,
            setting_row(
                true,
                "No Cloud plan yet",
                "Cloud plans include machine minutes. Usage shows up here as soon as a cloud computer runs.",
                Some(
                    button("usage-open-billing", "See plans", true, false, false)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            act(this, Action::OpenBilling, window, cx)
                        }))
                        .into_any_element(),
                ),
            ),
        ));
    }
    let Some(summary) = data.summary else {
        return root.child(section(
            "Cloud compute",
            "",
            None,
            setting_row(
                true,
                if data.pending.loading {
                    "Loading usage…"
                } else {
                    "Usage is unavailable"
                },
                "Machine minutes for this billing cycle.",
                (!data.pending.loading).then(|| {
                    button("retry-cloud-usage", "Retry", false, false, false)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            act(this, Action::Refresh, window, cx)
                        }))
                        .into_any_element()
                }),
            ),
        ));
    };
    let plan = summary.plan.as_ref();
    let by_minutes = plan.is_some();
    let rate = plan.map_or(DEFAULT_RATE, |p| p.default_machine_rate.max(1));
    let breakdown = breakdown(
        data.usage,
        summary.period_start,
        summary.period_end,
        by_minutes,
    );
    if let Some(banner) = plans::hold_banner(summary, data.pending.subscription, cx, act) {
        root = root.child(banner);
    }
    // The cycle at a glance.
    let left = days_left(summary.period_end);
    let cycle = stat(
        "This cycle",
        format!(
            "{} – {}",
            date(summary.period_start),
            date(summary.period_end)
        ),
        "",
        format!("{left} day{} left", if left == 1 { "" } else { "s" }),
    );
    let mut stats = div().flex().flex_wrap().gap(px(16.));
    let mut overview = div().flex().flex_col().gap(px(14.)).p(px(14.));
    match plan {
        Some(p) => {
            stats = stats
                .child(stat(
                    "Used",
                    grouped(p.used_machine_minutes),
                    "machine minutes",
                    format!(
                        "{} on {}",
                        about_hours(p.used_machine_minutes, rate),
                        plans::default_machine(p)
                    ),
                ))
                .child(stat(
                    "Left in your plan",
                    grouped(p.remaining_machine_minutes),
                    "machine minutes",
                    plans::meter_hint(p),
                ))
                .child(cycle);
            let fraction = share(p.used_machine_minutes, p.included_machine_minutes);
            overview = overview.child(stats).child(plans::meter(
                "usage-minutes-meter",
                fraction,
                8.,
                plans::meter_line(p),
            ));
            if let Some(extra) = plans::extra_line(p) {
                overview = overview.child(caption(extra));
            }
        }
        None => {
            stats = stats
                .child(stat(
                    "Compute and storage",
                    usd(breakdown.total_micros),
                    "",
                    if summary.usage_provisional {
                        "Still settling".into()
                    } else {
                        "This cycle so far".into()
                    },
                ))
                .child(stat(
                    if summary.shared_credits {
                        "Credits left"
                    } else {
                        "Included left"
                    },
                    usd(summary.included_remaining_micros),
                    "",
                    format!("of {}", usd(summary.included_provider_cost_micros)),
                ))
                .child(cycle);
            overview = overview.child(stats);
        }
    }
    root = root.child(section(
        "Cloud compute",
        "",
        Some(
            button("usage-manage-billing", "Billing", false, false, false)
                .on_click(cx.listener(move |this, _, window, cx| {
                    act(this, Action::OpenBilling, window, cx)
                }))
                .into_any_element(),
        ),
        overview,
    ));
    root = root.child(section(
        if by_minutes {
            "Machine minutes by day"
        } else {
            "Spend by day"
        },
        if by_minutes {
            "Hover a day for its hours. Asleep computers use none."
        } else {
            "Compute and storage for each day of this cycle."
        },
        None,
        chart(&breakdown, summary.period_start, by_minutes, rate),
    ));
    let total = if by_minutes {
        breakdown.total_minutes
    } else {
        breakdown.total_micros
    };
    let figure = |minutes: i64, cost: i64| {
        if by_minutes {
            (machine_minutes(minutes), share(minutes, total))
        } else {
            (usd(cost), share(cost, total))
        }
    };
    let mut table = div().flex().flex_col().child(table_header(if by_minutes {
        "Machine minutes"
    } else {
        "Spend"
    }));
    if breakdown.by_kind.is_empty() {
        table = table.child(setting_row(
            false,
            "No usage yet this cycle",
            "Start a cloud chat. Each chat's computer shows up here with the machine minutes it used.",
            None,
        ));
    }
    let limit = if data.history_open { usize::MAX } else { 8 };
    for (kind, lines) in &breakdown.by_kind {
        // Storage of asleep computers and saved images is included.
        if by_minutes && *kind == Kind::Storage {
            continue;
        }
        let cost: i64 = lines.iter().map(|l| l.cost_micros).sum();
        let minutes: i64 = lines.iter().map(|l| l.minutes).sum();
        let busy: i64 = lines.iter().map(|l| l.busy_ms).sum();
        let (used, part) = figure(minutes, cost);
        table = table.child(table_row(Row {
            id: format!("usage-kind-{}", *kind as u8),
            title: kind.label().into(),
            detail: None,
            time: if *kind == Kind::Storage {
                "—".into()
            } else {
                awake(busy)
            },
            spend: used,
            hint: by_minutes.then(|| {
                format!(
                    "{} on {}",
                    about_hours(minutes, rate),
                    machine_size(rate, rate * plan_math::UNIT_MEMORY_MIB)
                )
            }),
            share: Some(part),
            heading: true,
        }));
        for (i, line) in lines.iter().take(limit).enumerate() {
            let machine = machines.get(&line.resource_id);
            let size = (line.vcpu > 0).then(|| {
                let mut size = machine_size(
                    line.vcpu.clamp(0, u32::MAX as i64) as u32,
                    line.memory_mib.clamp(0, u32::MAX as i64) as u32,
                );
                if by_minutes && line.rate != DEFAULT_RATE && line.rate > 0 {
                    size = format!("{size} · {}", per_minute(line.rate));
                }
                size
            });
            let (title, detail) = match (kind, machine) {
                (Kind::Tasks, Some(m)) => (
                    m.title.clone(),
                    [Some(m.detail.clone()).filter(|d| !d.is_empty()), size]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                ),
                (Kind::Tasks, None) => ("Cloud computer".into(), size.unwrap_or_default()),
                (Kind::Builds, _) => ("Machine image build".into(), size.unwrap_or_default()),
                (Kind::Storage, _) => ("Saved machine image".into(), String::new()),
            };
            let (used, part) = figure(line.minutes, line.cost_micros);
            let line_rate = line.rate.max(1);
            table = table.child(table_row(Row {
                id: format!("usage-line-{}-{i}", *kind as u8),
                title: title + if line.provisional { " · running" } else { "" },
                detail: Some(detail).filter(|d| !d.is_empty()),
                time: if *kind == Kind::Storage {
                    "—".into()
                } else {
                    awake(line.busy_ms)
                },
                spend: used,
                hint: by_minutes.then(|| {
                    if line_rate == DEFAULT_RATE {
                        format!("{} awake", about_hours(line.minutes, line_rate))
                    } else {
                        format!(
                            "{} awake · uses {}",
                            about_hours(line.minutes, line_rate),
                            per_minute(line_rate)
                        )
                    }
                }),
                share: Some(part),
                heading: false,
            }));
        }
    }
    let hidden = breakdown.by_kind.values().any(|l| l.len() > 8);
    if hidden || data.more {
        table = table.child(
            div()
                .px(px(14.))
                .py(px(10.))
                .border_t_1()
                .border_color(palette::rgb(BORDER))
                .child(
                    button(
                        "toggle-cloud-usage-history",
                        if data.history_open {
                            "Show fewer"
                        } else {
                            "Show every chat"
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
    root.child(section(
        "By chat",
        if by_minutes {
            "Each chat runs on its own cloud computer. Hover machine minutes for hours."
        } else {
            "Each chat runs on its own cloud computer."
        },
        None,
        table,
    ))
    .child(caption(if by_minutes {
        "A machine minute is one minute of a 1 vCPU / 4 GB machine: a 4 vCPU / 16 GB computer uses 4 per awake minute, 8 vCPU / 32 GB uses 8. Storage for asleep computers and saved images is included. Model usage on your own subscriptions is under Models."
    } else {
        "Costs are Zuse's cost for each computer, before your plan's included allowance. Model usage on your own subscriptions is under Models."
    }).px(px(4.)))
}

fn status(data: &Data<'_>, plan: Option<&PlanUsage>) -> (&'static str, bool) {
    match (data.subscribed, data.summary) {
        (Some(true), Some(summary)) => match summary.status {
            _ if plan.is_some_and(|p| p.usage_hold) => ("Paused", true),
            _ if plan.is_some_and(|p| p.trial) => ("Trial", false),
            BillingStatus::Active | BillingStatus::Manual => ("Active", false),
            BillingStatus::Grace => ("Payment due", true),
            BillingStatus::BillingHold => ("Paused", true),
            BillingStatus::Ended => ("Ended", true),
        },
        (Some(true), None) => ("Active", false),
        (Some(false), _) => ("Not subscribed", false),
        (None, _) if data.pending.loading => ("Loading…", false),
        (None, _) => ("Unavailable", true),
    }
}

/// The cap editor row: what it limits and the field to change it.
fn cap_row<T: 'static>(
    data: &Data<'_>,
    cap: AnyElement,
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let disabled = data.pending.loading || data.pending.cap;
    setting_row(
        false,
        "Pay-as-you-go cap",
        "After your machine minutes, cloud computers keep running at the same rate up to this amount each month, before tax. They pause when it's reached.",
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .w(px(88.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .px(px(9.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(palette::rgb(BORDER))
                        .bg(s::frame_bg().opacity(0.4))
                        .child(caption("$"))
                        .child(div().flex_1().min_w_0().child(cap)),
                )
                .child(
                    button("save-cloud-cap", "Save", true, disabled, data.pending.cap).on_click(
                        cx.listener(move |this, _, window, cx| {
                            if !disabled {
                                act(this, Action::SaveCap, window, cx)
                            }
                        }),
                    ),
                )
                .into_any_element(),
        ),
    )
}

/// Billing: the plan and what is left, the cap, the plans to change to,
/// credits, who bills what and invoices.
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
    let subscribed = data.subscribed == Some(true);
    let plan = data
        .summary
        .and_then(|s| s.plan.as_ref())
        .filter(|_| subscribed);
    if let Some(banner) = data
        .summary
        .filter(|_| subscribed)
        .and_then(|summary| plans::hold_banner(summary, busy, cx, act))
    {
        root = root.child(banner);
    }
    let (label, warning) = status(data, plan);
    let (title, detail) = match (data.subscribed, data.summary, plan) {
        (_, _, Some(p)) => (plans::plan_title(p), plans::allowance_line(p)),
        (Some(true), Some(summary), None) => (
            format!("Cloud · {}/month", usd(summary.base_price_micros)),
            format!(
                "{} included each month",
                usd(summary.included_provider_cost_micros)
            ),
        ),
        (Some(true), None, None) => ("Cloud".into(), "Subscribed".into()),
        (Some(false), _, _) => (
            "No Cloud plan yet".into(),
            "Choose a plan below. Cloud computers use machine minutes only while they are awake."
                .into(),
        ),
        (None, _, _) => ("Cloud".into(), "Checking your plan…".into()),
    };
    let mut actions = div().flex().items_center().gap(px(6.));
    if let Some(p) = plan {
        actions = actions.child(caption(plans::renews_line(p)).mr(px(4.)));
    } else if let (Some(true), Some(summary)) = (data.subscribed, data.summary) {
        actions = actions.child(caption(format!("Renews {}", date(summary.period_end))).mr(px(4.)));
    }
    if plan.is_some_and(|p| p.trial) {
        actions = actions.child(
            button("start-cloud-plan", "Start Cloud now", true, busy, busy).on_click(cx.listener(
                move |this, _, window, cx| {
                    if !busy {
                        act(this, Action::Checkout(plan_math::CLOUD_OFFER), window, cx)
                    }
                },
            )),
        );
    }
    if data.subscribed.is_none() && !data.pending.loading {
        actions = actions.child(
            button("retry-cloud-subscription", "Retry", false, false, false).on_click(
                cx.listener(move |this, _, window, cx| act(this, Action::Refresh, window, cx)),
            ),
        );
    }
    if data.subscribed == Some(false) {
        return root
            .child(plans::plans_intro())
            .child(plans::plans_group(data, cx, act))
            .child(billing_footnote(cx, act));
    }
    let header = s::header_bar(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(s::title_text(title).truncate())
            .child(badge(label, warning, !subscribed)),
        Some(actions.into_any_element()),
    );
    let mut card = div()
        .flex()
        .flex_col()
        .child(s::row(true).child(s::description_text(detail).flex_1().min_w_0()));
    let mut pay_as_you_go = false;
    match (subscribed, data.summary, plan) {
        (true, _, Some(p)) => {
            pay_as_you_go = p.pay_as_you_go;
            card = card.child(
                s::row(false).child(
                    div().flex_1().min_w_0().child(plans::minutes_block(
                        p,
                        Some(
                            button("billing-open-usage", "See usage", false, false, false)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    act(this, Action::OpenUsage, window, cx)
                                }))
                                .into_any_element(),
                        ),
                    )),
                ),
            );
            if let Some(extra) = plans::extra_line(p) {
                card = card.child(setting_row(false, "Pay as you go", extra, None));
            } else if p.trial {
                card = card.child(setting_row(
                    false,
                    "No pay as you go during the trial",
                    "Cloud computers pause when the trial minutes are used up. Start Cloud now to keep going with 33,600 machine minutes a month.",
                    None,
                ));
            }
        }
        (true, Some(summary), None) => {
            pay_as_you_go = !summary.shared_credits;
            let included = summary.included_provider_cost_micros.max(1);
            let fraction = summary.included_used_micros as f32 / included as f32;
            card = card.child(
                s::row(false).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(8.))
                                .child(amount(usd(summary.included_remaining_micros), 28.))
                                .child(caption(if summary.shared_credits {
                                    "credits left"
                                } else {
                                    "included left"
                                })),
                        )
                        .child(plans::meter(
                            "cloud-included-meter",
                            fraction,
                            12.,
                            format!(
                                "{} of {} used",
                                usd(summary.included_used_micros),
                                usd(summary.included_provider_cost_micros)
                            ),
                        ))
                        .child(caption(format!(
                            "{} of {} used · invoice so far {} before tax",
                            usd(summary.included_used_micros),
                            usd(summary.included_provider_cost_micros),
                            usd(summary.current_invoice_estimate_micros)
                        ))),
                ),
            );
        }
        _ => {}
    }
    if pay_as_you_go {
        card = card.child(cap_row(data, cap, cx, act));
    }
    root = root.child(s::section(header, card));
    if data.subscribed.is_some() {
        root = root.child(plans::plans_group(data, cx, act));
    }
    if data
        .credits
        .is_some_and(|b| b.available || b.credit_cents > 0 || b.debit_cents > 0)
    {
        root = root.child(section(
            "Buy more minutes",
            "",
            Some(credits_header(data, cx, act)),
            credits_body(data, focus, false, cx, act),
        ));
    }
    let disabled = !subscribed || busy;
    root.child(
        s::frame().child(
            s::card().child(crate::cloud_settings::kit::row_item(
                true,
                None,
                "Invoices and payment method",
                "",
                Some(
                    icon_label_button(
                        "manage-cloud-subscription",
                        "arrow-up-right",
                        "Open billing portal",
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
        ),
    )
    .child(billing_footnote(cx, act))
}

/// Who bills what, in one quiet line: Zuse bills cloud computers; agents run
/// on the user's own subscriptions.
fn billing_footnote<T: 'static>(
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(4.))
        .child(crate::cloud_settings::kit::info_tip(
            "Who bills what",
            "Zuse bills cloud computers: your plan's machine minutes, then pay as you go up to your cap. Storage for asleep computers is included. Codex, Claude and Grok run on the subscription you sign in with, locally and in Cloud; Zuse never charges for their tokens.",
        ))
        .child(caption(
            "Zuse bills cloud computers. Agents run on your own subscriptions.",
        ))
        .child(
            div()
                .id("billing-open-agents")
                .role(Role::Button)
                .tab_index(0)
                .cursor_pointer()
                .text_size(px(11.))
                .text_color(palette::rgb(TEXT))
                .hover(|v| v.underline())
                .child("Agent accounts")
                .on_click(cx.listener(move |this, _, window, cx| {
                    act(this, Action::OpenAgents, window, cx)
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::{DAY_MS, Kind, awake, breakdown, percent};
    use zuse_protocol::cloud_billing::BillingUsageItem;
    fn item(kind: &str, id: &str, start: i64, minutes: i64, cost: i64) -> BillingUsageItem {
        BillingUsageItem {
            entry_id: format!("{id}-{start}"),
            resource_kind: kind.into(),
            usage_kind: None,
            resource_id: id.into(),
            provider: "cloud".into(),
            provider_execution_id: None,
            started_at: start,
            ended_at: start + minutes * 60_000,
            vcpu_count: 4,
            memory_mib: 16_384,
            provider_cost_micros: cost,
            status: "confirmed".into(),
            awake_ms: None,
            machine_rate: None,
            machine_minutes: None,
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
        let b = breakdown(&items, start, start + 30 * DAY_MS, false);
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
    #[test]
    fn machine_minutes_count_awake_time_times_the_machine_rate() {
        let start = 1_700_000_000_000;
        // 30 awake minutes on 8 vCPU / 32 GB use 8 per minute.
        let mut large = item("workspace", "w2", start, 60, 400);
        large.memory_mib = 32_768;
        large.awake_ms = Some(30 * 60_000);
        // The server's figure wins.
        let mut reported = item("workspace", "w1", start, 60, 100);
        reported.machine_minutes = Some(300);
        let mut storage = item("workspace", "w1", start, 60, 5);
        storage.usage_kind = Some("snapshot-storage".into());
        let b = breakdown(&[large, reported, storage], start, start + DAY_MS, true);
        assert_eq!(b.total_minutes, 540);
        assert_eq!(b.days[0], [540, 0, 0]);
        let tasks = &b.by_kind[&Kind::Tasks];
        assert_eq!(
            (tasks[0].resource_id.as_str(), tasks[0].minutes),
            ("w1", 300)
        );
        assert_eq!((tasks[1].minutes, tasks[1].rate), (240, 8));
        assert_eq!(b.by_kind[&Kind::Storage][0].minutes, 0);
    }
    #[test]
    fn small_figures_still_read() {
        assert_eq!(awake(0), "—");
        assert_eq!(awake(60_000), "<0.1 h");
        assert_eq!(awake(90 * 60_000), "1.5 h");
        assert_eq!(awake(12 * 3_600_000), "12 h");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.256), "26%");
    }
}
