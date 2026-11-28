//! Cloud plans in machine minutes: plan copy, the plan comparison, the trial
//! offer, the minutes meter and holds. Machine minutes are the headline,
//! hours a hint; dollars appear only on prices and pay as you go.
use super::*;
use crate::tooltip::TooltipExt as _;
use zuse_model::cloud_billing::{
    about_hours, dollars, grouped, machine_minutes, machine_size, monthly,
};
use zuse_protocol::cloud_billing::plans::{self, PlanCatalog, PlanId, PlanOffer};
use zuse_protocol::cloud_billing::{HoldReason, PlanUsage};

pub const TRIAL_COPY: &str = "Try Cloud for $1 for a week — 8,400 machine minutes (about 35 h on a 4 vCPU / 16 GB computer), then $20/month for 33,600.";
pub const ENTERPRISE_COPY: &str =
    "More machine minutes, larger computers, invoicing and support for your team.";

/// The server's catalog, or the published plans from older servers.
pub fn catalog(catalog: Option<&PlanCatalog>) -> PlanCatalog {
    catalog
        .cloned()
        .unwrap_or_else(|| plans::catalog(false, &|_| true, plans::ENTERPRISE_CONTACT_FALLBACK))
}
pub(super) fn date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%b %-d").to_string())
        .unwrap_or_default()
}
pub fn plan_id(summary: Option<&BillingSummary>) -> Option<PlanId> {
    Some(summary?.plan.as_ref()?.plan_id)
}
/// The machine a plan's hours are counted on: "4 vCPU / 16 GB".
pub fn default_machine(p: &PlanUsage) -> String {
    let rate = p.default_machine_rate.max(1);
    machine_size(rate, rate * plans::UNIT_MEMORY_MIB)
}
/// "Cloud · $20/month".
pub fn plan_title(p: &PlanUsage) -> String {
    if p.trial {
        format!(
            "Cloud trial · {} for the first week",
            dollars(p.price_micros)
        )
    } else {
        format!("{} · {}", p.plan_name, monthly(p.price_micros))
    }
}
/// "33,600 machine minutes · about 140 h on your 4 vCPU / 16 GB machine".
pub fn allowance_line(p: &PlanUsage) -> String {
    format!(
        "{} · {} on your {} machine",
        machine_minutes(p.included_machine_minutes),
        about_hours(p.included_machine_minutes, p.default_machine_rate),
        default_machine(p)
    )
}
/// "Renews Dec 1", or when a trial becomes Cloud.
pub fn renews_line(p: &PlanUsage) -> String {
    if p.trial {
        format!(
            "Cloud starts {} at $20/month",
            date(p.trial_ends_at.unwrap_or(p.renews_at))
        )
    } else {
        format!("Renews {}", date(p.renews_at))
    }
}
/// "12,480 of 33,600 machine minutes used · 21,120 left".
pub fn meter_line(p: &PlanUsage) -> String {
    format!(
        "{} of {} used · {} left",
        grouped(p.used_machine_minutes.min(p.included_machine_minutes)),
        machine_minutes(p.included_machine_minutes),
        grouped(p.remaining_machine_minutes)
    )
}
/// "about 88 h left on 4 vCPU / 16 GB".
pub fn meter_hint(p: &PlanUsage) -> String {
    format!(
        "{} left on {}",
        about_hours(p.remaining_machine_minutes, p.default_machine_rate),
        default_machine(p)
    )
}
/// "2,880 extra machine minutes this month ($1.71) · cap $25"; none when zero.
pub fn extra_line(p: &PlanUsage) -> Option<String> {
    (p.extra_machine_minutes > 0).then(|| {
        format!(
            "{} extra machine minutes this month ({}) · cap {}",
            grouped(p.extra_machine_minutes),
            usd(p.extra_charge_micros),
            dollars(p.overage_cap_micros)
        )
    })
}
/// "Trial · ends Nov 26 · 3,200 of 8,400 machine minutes left".
pub fn trial_line(p: &PlanUsage) -> String {
    let mut parts = vec!["Trial".to_owned()];
    if let Some(end) = p.trial_ends_at {
        parts.push(format!("ends {}", date(end)));
    }
    parts.push(format!(
        "{} of {} left",
        grouped(p.remaining_machine_minutes),
        machine_minutes(p.included_machine_minutes)
    ));
    parts.join(" · ")
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldButton {
    StartCloud,
    RaiseCap,
    UpgradeMax,
}
impl HoldButton {
    pub fn label(self) -> &'static str {
        match self {
            Self::StartCloud => "Start Cloud now",
            Self::RaiseCap => "Raise cap",
            Self::UpgradeMax => "Upgrade to Cloud Max",
        }
    }
    fn action(self) -> Action {
        match self {
            Self::StartCloud => Action::Checkout(plans::CLOUD_OFFER),
            Self::RaiseCap => Action::RaiseCap,
            Self::UpgradeMax => Action::Checkout(plans::CLOUD_MAX_OFFER),
        }
    }
}
/// The hold message and the buttons that clear it.
pub fn hold(p: &PlanUsage) -> Option<(String, Vec<HoldButton>)> {
    if !p.usage_hold {
        return None;
    }
    Some(match p.hold_reason? {
        HoldReason::TrialAllowanceUsed => (
            "Your trial minutes are used up. Cloud computers are paused.".into(),
            vec![HoldButton::StartCloud],
        ),
        HoldReason::TrialUnavailable => (
            "This card or account has already used a trial.".into(),
            vec![HoldButton::StartCloud],
        ),
        HoldReason::CapReached => {
            let mut buttons = vec![HoldButton::RaiseCap];
            if p.plan_id != PlanId::CloudMax {
                buttons.push(HoldButton::UpgradeMax);
            }
            (
                format!(
                    "You reached your {} pay-as-you-go cap. Cloud computers are paused until {}.",
                    dollars(p.overage_cap_micros),
                    date(p.renews_at)
                ),
                buttons,
            )
        }
    })
}
fn warning_tone() -> Hsla {
    hsla(21.6 / 360., 0.982, 0.567, 1.)
}
fn lime() -> Hsla {
    palette::accent_or(palette::tone(
        hsla(83. / 360., 0.72, 0.46, 1.),
        hsla(83. / 360., 0.74, 0.40, 1.),
    ))
}
pub(super) fn hold_banner<T: 'static>(
    summary: &BillingSummary,
    busy: bool,
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Option<Stateful<Div>> {
    let (text, buttons) = hold(summary.plan.as_ref()?)?;
    let mut controls = div().flex().items_center().gap(px(6.));
    for (i, kind) in buttons.into_iter().enumerate() {
        let action = kind.action();
        let checkout = matches!(action, Action::Checkout(_));
        let disabled = checkout && busy;
        controls = controls.child(
            button(
                ("cloud-hold-action", i),
                kind.label(),
                i == 0,
                disabled,
                disabled,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if !disabled {
                    act(this, action, window, cx)
                }
            })),
        );
    }
    Some(
        div()
            .id("cloud-usage-hold")
            .role(Role::Alert)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .rounded(px(s::FRAME_RADIUS))
            .border_1()
            .border_color(s::destructive_text().opacity(0.3))
            .bg(palette::tone(
                hsla(349.3 / 360., 0.311, 0.14, 1.),
                hsla(0., 1., 0.975, 1.),
            ))
            .px(px(14.))
            .py(px(12.))
            .child(
                div()
                    .size(px(8.))
                    .flex_none()
                    .rounded_full()
                    .bg(s::destructive_text()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(200.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette::rgb(TEXT))
                            .child("Cloud computers are paused"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(s::destructive_text())
                            .child(text),
                    ),
            )
            .child(controls),
    )
}
/// A dithered meter `height` tall filled to `fraction`; amber from 90%.
pub(super) fn meter(id: &'static str, fraction: f32, height: f32, label: String) -> Stateful<Div> {
    let fill = if fraction >= 0.9 {
        warning_tone()
    } else {
        lime()
    };
    div()
        .id(id)
        .role(Role::ProgressIndicator)
        .aria_label(SharedString::from(label))
        .relative()
        .w_full()
        .h(px(height))
        .child(crate::dither::meter(
            fraction.clamp(0., 1.),
            fill,
            palette::rgb(MUTED).opacity(0.35).into(),
        ))
}
/// The big number, the dithered meter and the used/left line, with an
/// optional control at the line's end.
pub(super) fn minutes_block(p: &PlanUsage, trailing: Option<AnyElement>) -> Div {
    let included = p.included_machine_minutes.max(1);
    let fraction = p.used_machine_minutes as f32 / included as f32;
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(28.))
                                .line_height(px(32.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette::rgb(TEXT))
                                .child(grouped(p.remaining_machine_minutes)),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(palette::rgb(MUTED))
                                .child("machine minutes left"),
                        ),
                )
                .child(
                    div()
                        .id("cloud-minutes-hint")
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(palette::rgb(MUTED))
                        .child(meter_hint(p))
                        .tip(
                            "Hours are awake time on your default computer. Larger computers use machine minutes faster; asleep computers use none.",
                        ),
                ),
        )
        .child(meter("cloud-minutes-meter", fraction, 12., meter_line(p)))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .min_h(px(16.))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(palette::rgb(MUTED))
                        .child(if p.trial { trial_line(p) } else { meter_line(p) }),
                )
                .children(trailing),
        )
}
/// What a plan card's control does for this account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanControl {
    Current(&'static str),
    Checkout {
        label: &'static str,
        offer: &'static str,
        primary: bool,
    },
}
pub fn plan_control(offer: &PlanOffer, current: Option<PlanId>, subscribed: bool) -> PlanControl {
    let id = if offer.plan_id == PlanId::CloudMax {
        plans::CLOUD_MAX_OFFER
    } else {
        plans::CLOUD_OFFER
    };
    let current = current.filter(|_| subscribed);
    match (current, offer.plan_id) {
        (Some(PlanId::CloudTrial), PlanId::Cloud) => PlanControl::Checkout {
            label: "Start Cloud now",
            offer: id,
            primary: true,
        },
        (Some(PlanId::Cloud | PlanId::Legacy), PlanId::Cloud) => {
            PlanControl::Current("Current plan")
        }
        (Some(PlanId::CloudMax), PlanId::CloudMax) => PlanControl::Current("Current plan"),
        (Some(PlanId::CloudMax), _) => PlanControl::Checkout {
            label: "Switch to Cloud",
            offer: id,
            primary: false,
        },
        (Some(_), PlanId::CloudMax) => PlanControl::Checkout {
            label: "Upgrade to Cloud Max",
            offer: id,
            primary: true,
        },
        // Subscribed before plans were reported: let the portal decide.
        (None, _) if subscribed => PlanControl::Checkout {
            label: "Switch plan",
            offer: id,
            primary: false,
        },
        (_, PlanId::CloudMax) => PlanControl::Checkout {
            label: "Get Cloud Max",
            offer: id,
            primary: true,
        },
        _ => PlanControl::Checkout {
            label: "Get Cloud",
            offer: id,
            primary: true,
        },
    }
}
/// What each plan includes, one line each.
pub fn plan_features(offer: &PlanOffer) -> Vec<String> {
    let rate = plans::DEFAULT_RATE;
    let mut lines = vec![format!(
        "{} on a 4 vCPU / 16 GB computer",
        about_hours(offer.included_machine_minutes, rate)
    )];
    lines.push(if offer.max_vcpus > plans::DEFAULT_VCPUS {
        format!(
            "Computers up to {}",
            machine_size(offer.max_vcpus, offer.max_memory_mib)
        )
    } else {
        "Standard 4 vCPU / 16 GB computers".into()
    });
    if offer.pay_as_you_go {
        lines.push("Pay as you go after, up to your cap".into());
    }
    lines.push("Asleep computers use nothing".into());
    lines
}
/// The icon beside a feature line.
fn feature_glyph(line: &str) -> &'static str {
    if line.starts_with("about") {
        "clock-circle"
    } else if line.starts_with("Asleep") {
        "moon"
    } else if line.contains("vCPU") || line.contains("computers") {
        "monitor"
    } else if line.starts_with("Pay as you go") {
        "dashboard-speed"
    } else if line.starts_with("Asleep") {
        "moon"
    } else {
        "check"
    }
}
/// The price block of a plan card.
pub struct Price {
    pub was: Option<String>,
    pub amount: String,
    pub unit: &'static str,
    pub note: String,
}
/// One plan as the shared settings double card: the plan's name (and its
/// marker) in the frame header; price, minutes, the action and the feature
/// list as rows of the inner card. Neutral colours throughout.
#[allow(clippy::too_many_arguments)]
fn plan_card(
    tag: Option<&'static str>,
    name: String,
    price: Price,
    usage: String,
    control: AnyElement,
    today: Option<String>,
    intro: Option<&'static str>,
    features: Vec<String>,
    accent: bool,
) -> Div {
    // The marker sits in the title row rather than the 28px action slot, so
    // every plan's header keeps the plain title height.
    let header = s::header_bar(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(s::title_text(name))
            .when_some(tag, |v, tag| {
                v.child(
                    crate::cloud_settings::kit::pill(
                        tag,
                        if accent {
                            crate::cloud_settings::kit::Tone::Ready
                        } else {
                            crate::cloud_settings::kit::Tone::Neutral
                        },
                    )
                    .my(px(-1.)),
                )
            }),
        None,
    );
    let inner = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .px(px(s::ROW_PAD_X))
        .pt(px(14.))
        .pb(px(10.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(26.))
                                .line_height(px(30.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette::rgb(TEXT))
                                .child(price.amount),
                        )
                        .child(s::description_text(price.unit))
                        .when_some(price.was, |v, was| {
                            v.child(
                                div()
                                    .text_size(px(11.))
                                    .line_through()
                                    .text_color(palette::rgb(MUTED))
                                    .child(was),
                            )
                        }),
                )
                .child(s::description_text(price.note)),
        )
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette::rgb(TEXT))
                .child(usage),
        )
        .child(
            div().flex().flex_col().gap(px(8.)).child(control).child(
                div()
                    .h(px(16.))
                    .flex()
                    .justify_center()
                    .when_some(today, |v, t| v.child(s::description_text(t))),
            ),
        );
    let mut list = div()
        .flex()
        .flex_col()
        .gap(px(9.))
        .px(px(s::ROW_PAD_X))
        .py(px(s::ROW_PAD_Y))
        .border_t_1()
        .border_color(palette::rgb(BORDER));
    if let Some(intro) = intro {
        list = list.child(s::description_text(intro));
    }
    for line in features {
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    crate::assets::icon(feature_glyph(&line))
                        .mt(px(2.))
                        .size(px(12.))
                        .text_color(palette::rgb(MUTED)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette::rgb(TEXT))
                        .child(line),
                ),
        );
    }
    // `s::section`, with the inner card stretched so side-by-side plans
    // share one height.
    s::frame()
        .flex_1()
        .min_w(px(200.))
        .child(header)
        .child(s::card().flex_grow(1.).child(inner).child(list))
}
/// One line under the page header for an account without a plan.
pub(super) fn plans_intro() -> Div {
    s::description_text(
        "Choose a plan. Every plan includes machine minutes each month, and asleep computers use none.",
    )
}
/// Cloud, Cloud Max and Enterprise side by side. While the $1 trial is
/// offered it lives in the Cloud card.
pub(super) fn plans_group<T: 'static>(
    data: &Data<'_>,
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let catalog = catalog(data.catalog);
    let current = plan_id(data.summary);
    let subscribed = data.subscribed == Some(true);
    let trial = catalog.trial_available && data.subscribed == Some(false);
    let busy = data.pending.subscription;
    let mut cards = div().flex().flex_wrap().gap(px(10.));
    let mut previous: Option<String> = None;
    for offer in catalog
        .plans
        .iter()
        .filter(|o| matches!(o.plan_id, PlanId::Cloud | PlanId::CloudMax))
    {
        let control = plan_control(offer, current, subscribed);
        let is_current = matches!(control, PlanControl::Current(_));
        let cloud_trial = trial && offer.plan_id == PlanId::Cloud;
        let element = match control {
            PlanControl::Current(label) => div()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .border_1()
                .border_color(s::card_border())
                .text_size(px(11.))
                .text_color(palette::rgb(MUTED))
                .child(label)
                .into_any_element(),
            PlanControl::Checkout {
                label,
                offer: offer_id,
                ..
            } => {
                let (label, offer_id) = if cloud_trial {
                    ("Start for $1", plans::CLOUD_TRIAL_OFFER)
                } else {
                    (label, offer_id)
                };
                let disabled = busy || !offer.available || data.subscribed.is_none();
                crate::cloud_settings::kit::primary_button(
                    SharedString::from(format!("cloud-plan-{offer_id}")),
                    label,
                    !disabled,
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !disabled {
                        act(this, Action::Checkout(offer_id), window, cx)
                    }
                }))
                .into_any_element()
            }
        };
        let monthly_price = dollars(offer.price_micros);
        let price = if cloud_trial {
            Price {
                was: Some(monthly_price.clone()),
                amount: "$1".into(),
                unit: "first 7 days",
                note: format!("then {monthly_price}/mo"),
            }
        } else {
            Price {
                was: None,
                amount: monthly_price.clone(),
                unit: "/mo",
                note: "billed monthly".into(),
            }
        };
        let (intro, features) = match &previous {
            Some(_) if offer.plan_id == PlanId::CloudMax => (
                Some("Everything in Cloud, plus"),
                plan_features(offer).into_iter().take(2).collect::<Vec<_>>(),
            ),
            _ => (None, plan_features(offer)),
        };
        previous = Some(offer.name.clone());
        let recommended = !subscribed && offer.plan_id == PlanId::Cloud;
        cards = cards.child(plan_card(
            if is_current {
                Some("Current plan")
            } else if cloud_trial {
                Some("$1 first week")
            } else if recommended {
                Some("Recommended")
            } else {
                None
            },
            offer.name.clone(),
            price,
            format!("{} / mo", machine_minutes(offer.included_machine_minutes)),
            element,
            cloud_trial.then(|| format!("Today: $1, then {monthly_price}/mo")),
            intro,
            features,
            is_current || recommended,
        ));
    }
    let url = catalog.enterprise_contact_url.clone();
    let valid = url.starts_with("https://") || url.starts_with("mailto:");
    cards = cards.child(plan_card(
        None,
        "Enterprise".into(),
        Price {
            was: None,
            amount: "Custom".into(),
            unit: "",
            note: "for your team".into(),
        },
        "Machine minutes for your team".into(),
        crate::cloud_settings::kit::primary_button("cloud-plan-enterprise", "Contact us", valid)
            .w_full()
            .on_click(cx.listener(move |_, _, _, cx| {
                if valid {
                    cx.open_url(&url)
                }
            }))
            .into_any_element(),
        None,
        Some("Everything in Cloud Max, plus"),
        vec![ENTERPRISE_COPY.into(), "Larger computers on request".into()],
        false,
    ));
    cards
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn usage(plan: PlanId) -> PlanUsage {
        PlanUsage {
            plan_id: plan,
            plan_name: "Cloud".into(),
            price_micros: 20_000_000,
            trial: false,
            trial_ends_at: None,
            renews_at: 0,
            included_machine_minutes: 33_600,
            used_machine_minutes: 12_480,
            remaining_machine_minutes: 21_120,
            extra_machine_minutes: 2_880,
            extra_charge_micros: 1_714_286,
            overage_cap_micros: 25_000_000,
            pay_as_you_go: true,
            usage_hold: false,
            hold_reason: None,
            default_machine_rate: 4,
            max_vcpus: 4,
            max_memory_mib: 16_384,
        }
    }
    #[test]
    fn plan_lines_follow_the_published_copy() {
        let mut p = usage(PlanId::Cloud);
        assert_eq!(plan_title(&p), "Cloud · $20/month");
        assert_eq!(
            allowance_line(&p),
            "33,600 machine minutes · about 140 h on your 4 vCPU / 16 GB machine"
        );
        assert!(renews_line(&p).starts_with("Renews "));
        assert_eq!(
            meter_line(&p),
            "12,480 of 33,600 machine minutes used · 21,120 left"
        );
        assert_eq!(meter_hint(&p), "about 88 h left on 4 vCPU / 16 GB");
        assert_eq!(
            extra_line(&p).unwrap(),
            "2,880 extra machine minutes this month ($1.71) · cap $25"
        );
        p.extra_machine_minutes = 0;
        assert_eq!(extra_line(&p), None);
        p.trial = true;
        p.included_machine_minutes = 8_400;
        p.remaining_machine_minutes = 3_200;
        assert_eq!(
            trial_line(&p),
            "Trial · 3,200 of 8,400 machine minutes left"
        );
        p.trial_ends_at = Some(1_700_000_000_000);
        assert!(trial_line(&p).starts_with("Trial · ends "));
    }
    #[test]
    fn holds_offer_the_way_out() {
        let mut p = usage(PlanId::Cloud);
        assert_eq!(hold(&p), None);
        p.usage_hold = true;
        p.hold_reason = Some(HoldReason::CapReached);
        let (text, buttons) = hold(&p).unwrap();
        assert!(text.starts_with(
            "You reached your $25 pay-as-you-go cap. Cloud computers are paused until "
        ));
        assert_eq!(buttons, [HoldButton::RaiseCap, HoldButton::UpgradeMax]);
        p.plan_id = PlanId::CloudMax;
        assert_eq!(hold(&p).unwrap().1, [HoldButton::RaiseCap]);
        p.hold_reason = Some(HoldReason::TrialAllowanceUsed);
        assert_eq!(
            hold(&p).unwrap(),
            (
                "Your trial minutes are used up. Cloud computers are paused.".into(),
                vec![HoldButton::StartCloud]
            )
        );
        p.hold_reason = Some(HoldReason::TrialUnavailable);
        assert_eq!(
            hold(&p).unwrap().0,
            "This card or account has already used a trial."
        );
        let labels = [
            HoldButton::StartCloud,
            HoldButton::RaiseCap,
            HoldButton::UpgradeMax,
        ]
        .map(HoldButton::label);
        assert_eq!(
            labels,
            ["Start Cloud now", "Raise cap", "Upgrade to Cloud Max"]
        );
    }
    #[test]
    fn plan_cards_offer_the_right_change() {
        let c = catalog(None);
        let (cloud, max) = (&c.plans[0], &c.plans[1]);
        assert_eq!(
            (cloud.name.as_str(), max.name.as_str()),
            ("Cloud", "Cloud Max")
        );
        assert_eq!(
            machine_minutes(max.included_machine_minutes),
            "84,000 machine minutes"
        );
        let label = |offer, current, subscribed| match plan_control(offer, current, subscribed) {
            PlanControl::Current(l) | PlanControl::Checkout { label: l, .. } => l,
        };
        assert_eq!(label(cloud, None, false), "Get Cloud");
        assert_eq!(label(max, None, false), "Get Cloud Max");
        assert_eq!(label(cloud, Some(PlanId::Legacy), true), "Current plan");
        assert_eq!(
            label(max, Some(PlanId::Cloud), true),
            "Upgrade to Cloud Max"
        );
        assert_eq!(
            label(cloud, Some(PlanId::CloudTrial), true),
            "Start Cloud now"
        );
        assert_eq!(
            label(cloud, Some(PlanId::CloudMax), true),
            "Switch to Cloud"
        );
        assert_eq!(label(max, Some(PlanId::CloudMax), true), "Current plan");
        assert_eq!(
            plan_features(max),
            [
                "about 350 h on a 4 vCPU / 16 GB computer",
                "Computers up to 8 vCPU / 32 GB",
                "Pay as you go after, up to your cap",
                "Asleep computers use nothing"
            ]
        );
        assert!(!c.trial_available);
        let p = usage(PlanId::Cloud);
        let texts = c.plans.iter().flat_map(plan_features).chain([
            TRIAL_COPY.into(),
            ENTERPRISE_COPY.into(),
            plan_title(&p),
            allowance_line(&p),
            meter_line(&p),
            meter_hint(&p),
            trial_line(&p),
            extra_line(&p).unwrap(),
        ]);
        for text in texts {
            assert!(!plans::names_infrastructure(&text), "{text}");
        }
    }
}
