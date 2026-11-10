//! Shared Cloud hosting and billing presentation. Hosts retain account-bound RPC ownership.
use crate::cloud_settings::style::{badge, button, group, note, setting_row};
use crate::localization::message as tr;
use crate::{
    palette, settings_style as s,
    shell::{BORDER, MUTED, TEXT},
};
use gpui::{prelude::*, *};
use serde_json::Value;
use zuse_i18n::messages as msg;
use zuse_model::cloud_billing::{cents, usd};
mod pages;
pub use pages::{Machine, billing, compute};
use zuse_protocol::cloud_billing::{
    BillingStatus, BillingSummary, BillingUsageItem, PrepaidBalance,
};

#[derive(Clone, Copy)]
pub enum Action {
    Refresh,
    RefreshCredits,
    Subscription,
    ManageSubscription,
    SaveCap,
    SelectCredit(i64),
    BuyCredits,
    MoreUsage,
    ToggleUsageHistory,
    /// Desktop pages link to each other; the browser ignores these.
    OpenBilling,
    OpenUsage,
    OpenAgents,
}
#[derive(Clone, Copy, Default)]
pub struct Pending {
    pub loading: bool,
    pub subscription: bool,
    pub credits: bool,
    pub cap: bool,
}
pub struct Data<'a> {
    pub subscribed: Option<bool>,
    pub pending: Pending,
    pub summary: Option<&'a BillingSummary>,
    pub usage: &'a [BillingUsageItem],
    pub credits: Option<&'a PrepaidBalance>,
    pub credit_amount: i64,
    pub error: Option<&'a str>,
    pub credit_error: Option<&'a str>,
    pub more: bool,
    pub history_open: bool,
    pub workspaces: &'a [Value],
    pub personal: bool,
}
pub fn hosting<T: 'static>(
    data: &Data<'_>,
    workspace: &str,
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let busy = data.pending.subscription;
    let disabled = data.subscribed.is_none() || busy;
    let mut controls = div().flex().items_center().gap(px(6.));
    if data.subscribed == Some(true) {
        controls = controls.child(badge("In use", false, false));
    }
    let label = if data.subscribed == Some(true) {
        "Manage subscription"
    } else {
        "Subscribe · $40/month"
    };
    if data.subscribed.is_none() {
        controls = controls.child(badge(
            if data.pending.loading {
                "Loading…"
            } else {
                "Could not load billing"
            },
            false,
            true,
        ));
        if !data.pending.loading {
            controls = controls.child(
                button("retry-cloud-subscription", "Retry", false, false, false).on_click(
                    cx.listener(move |this, _, window, cx| act(this, Action::Refresh, window, cx)),
                ),
            );
        }
    } else {
        controls = controls.child(
            button(
                "open-cloud-billing-portal",
                label,
                data.subscribed == Some(false),
                disabled,
                busy,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if !disabled {
                    act(this, Action::Subscription, window, cx)
                }
            })),
        );
    }
    let mut identity = div()
        .h(px(28.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(s::title_text(SharedString::from(format!(
            "Cloud · {workspace}"
        ))));
    if data.subscribed == Some(true) {
        identity = identity.child(badge("Ready", false, false));
    }
    let body = div().child(setting_row(
        true,
        "Zuse Cloud",
        if data.subscribed == Some(true) {
            "Subscribed. Zuse runs and bills the machines."
        } else {
            "No setup. Zuse runs and bills the machines."
        },
        Some(controls.into_any_element()),
    ));
    let mut root = div().flex().flex_col().gap(px(16.)).child(identity);
    if let Some(error) = data.error {
        root = root.child(
            div()
                .id("cloud-billing-error")
                .role(Role::Alert)
                .rounded(px(7.))
                .px(px(12.))
                .py(px(8.))
                .bg(palette::tone(
                    hsla(349.3 / 360., 0.311, 0.176, 1.),
                    hsla(0., 1., 0.973, 1.),
                ))
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(s::destructive_text())
                .child(error.to_owned()),
        );
    }
    root.child(group("Hosting",Some("Hosting pays for the machines. Agents and GitHub use your own accounts, connected through Zuse or already signed in on your snapshot."),None,body))
}

pub fn usage<T: 'static>(
    data: &Data<'_>,
    cap: AnyElement,
    focus: &[FocusHandle; 4],
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let mut root = div().flex().flex_col().gap(px(16.));
    if data.subscribed == Some(true) {
        let body = if let Some(summary) = data.summary {
            let percent = if summary.overage_cap_micros <= 0 {
                if summary.overage_provider_cost_micros > 0 {
                    100
                } else {
                    0
                }
            } else {
                ((i128::from(summary.overage_charge_micros) * 100
                    / i128::from(summary.overage_cap_micros))
                .clamp(0, 100)) as i64
            };
            let mut rows = div().flex().flex_col();
            let mut first = true;
            if percent >= 50 {
                rows=rows.child(setting_row(first,if percent>=100{"Billing hold"}else{"Usage warning"},if percent>=100{"The overage cap is exhausted. New billable operations are blocked and running compute is being paused.".to_owned()}else{format!("You have used {percent}% of this period's overage cap.")},Some(badge(SharedString::from(format!("{percent}%")),true,false).into_any_element())));
                first = false;
            }
            rows = rows.child(setting_row(
                first,
                SharedString::from(format!(
                    "{} of {} included used",
                    usd(summary.included_used_micros),
                    usd(summary.included_provider_cost_micros)
                )),
                SharedString::from(format!(
                    "{} remaining · {} overage · {} invoice estimate before tax",
                    usd(summary.included_remaining_micros),
                    usd(summary.overage_charge_micros),
                    usd(summary.current_invoice_estimate_micros)
                )),
                None,
            ));
            let busy = data.pending.cap;
            let disabled = data.pending.loading || busy;
            rows = rows.child(setting_row(
                false,
                "Monthly overage cap",
                "Builds and running workspaces pause when this pre-tax limit is reached.",
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
                            button("save-cloud-cap", "Save", true, disabled, busy).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    if !disabled {
                                        act(this, Action::SaveCap, window, cx)
                                    }
                                }),
                            ),
                        )
                        .into_any_element(),
                ),
            ));
            let recent = if data.usage.is_empty() {
                "No completed sandbox runs in this billing period.".to_owned()
            } else {
                data.usage
                    .iter()
                    .take(3)
                    .map(|item| {
                        format!(
                            "{} {}: {}{}",
                            item.usage_kind.as_deref().unwrap_or(&item.resource_kind),
                            item.resource_id,
                            usd(item.provider_cost_micros),
                            if item.status == "provisional" {
                                " (provisional)"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            rows = rows.child(setting_row(
                false,
                "Recent usage",
                SharedString::from(recent),
                None,
            ));
            if data.usage.len() > 3 || data.more {
                rows = rows.child(
                    div().px(px(12.)).pb(px(10.)).child(
                        button(
                            "toggle-cloud-usage-history",
                            if data.history_open {
                                "Hide history"
                            } else {
                                "View history"
                            },
                            false,
                            false,
                            false,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                act(this, Action::ToggleUsageHistory, window, cx)
                            },
                        )),
                    ),
                );
            }
            if data.history_open {
                for item in data.usage.iter().skip(3) {
                    rows = rows.child(setting_row(
                        false,
                        SharedString::from(format!(
                            "{} {}",
                            item.usage_kind.as_deref().unwrap_or(&item.resource_kind),
                            item.resource_id
                        )),
                        SharedString::from(format!(
                            "{} · {} · {}",
                            usd(item.provider_cost_micros),
                            item.provider,
                            item.status
                        )),
                        None,
                    ));
                }
                if data.more {
                    let disabled = data.pending.loading;
                    rows = rows.child(div().px(px(12.)).pb(px(10.)).child(
                        button("more-cloud-usage", "Load more", false, disabled, false).on_click(
                            cx.listener(move |this, _, window, cx| {
                                if !disabled {
                                    act(this, Action::MoreUsage, window, cx)
                                }
                            }),
                        ),
                    ));
                }
            }
            let paused = summary.status == BillingStatus::BillingHold;
            group(
                "Usage and billing",
                Some(
                    "$40/month includes $35 of sandbox compute and image storage. Additional usage has a 5% markup.",
                ),
                Some(
                    badge(if paused { "Paused" } else { "Active" }, paused, false)
                        .into_any_element(),
                ),
                rows,
            )
        } else {
            let disabled = data.pending.loading;
            group(
                "Usage and billing",
                Some("Usage details are temporarily unavailable."),
                None,
                div().child(setting_row(
                    true,
                    if disabled {
                        "Loading billing…"
                    } else {
                        "Could not load billing"
                    },
                    "Refresh cloud settings to try again. Existing workspaces are unaffected.",
                    Some(
                        button("retry-cloud-billing", "Retry", false, disabled, false)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !disabled {
                                    act(this, Action::Refresh, window, cx)
                                }
                            }))
                            .into_any_element(),
                    ),
                )),
            )
        };
        root = root.child(body);
    }
    let credits = credits_body(data, focus, cx, act);
    let portal_busy = data.pending.subscription;
    let disabled = portal_busy;
    let billing=div().flex().flex_col().child(credits).child(setting_row(false,"Manage subscription","Manage or cancel your Zuse subscription, payment details, and invoices in the billing portal.",Some(button("manage-cloud-subscription","Manage subscription",false,disabled,portal_busy).on_click(cx.listener(move|this,_,window,cx|if !disabled{act(this,Action::ManageSubscription,window,cx)})).into_any_element())));
    root = root.child(group("Billing", None, None, billing));
    if data.subscribed == Some(true) && data.personal {
        let mut activity = div().flex().flex_col();
        if data.workspaces.is_empty() {
            activity = activity.child(setting_row(
                true,
                "No cloud workspaces yet",
                "Start a cloud chat and its workspace will appear here.",
                None,
            ));
        } else {
            for (i, workspace) in data.workspaces.iter().enumerate() {
                let state = workspace["state"].as_str().unwrap_or("unknown");
                activity = activity.child(setting_row(
                    i == 0,
                    SharedString::from(
                        workspace["branch"]
                            .as_str()
                            .unwrap_or("Workspace")
                            .to_owned(),
                    ),
                    SharedString::from(workspace["statusCode"].as_str().unwrap_or("").to_owned()),
                    Some(
                        badge(
                            SharedString::from(state.to_owned()),
                            !matches!(state, "ready" | "deleted"),
                            state == "deleted",
                        )
                        .into_any_element(),
                    ),
                ));
            }
        }
        root = root.child(group(
            "Workspace activity",
            Some("Current and recent cloud workspaces for this account."),
            Some(
                badge(
                    SharedString::from(data.workspaces.len().to_string()),
                    false,
                    true,
                )
                .into_any_element(),
            ),
            activity,
        ));
    }
    root
}

/// Zuse credits: balance, amount picker and purchase. Shared by both layouts.
fn credits_body<T: 'static>(
    data: &Data<'_>,
    focus: &[FocusHandle; 4],
    cx: &mut Context<T>,
    act: impl Fn(&mut T, Action, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Div {
    let busy = data.pending.credits;
    let enabled = data.credits.is_some_and(|b| b.available) && !busy;
    let mut credits = div().flex().flex_col().gap(px(12.)).p(px(12.));
    credits = credits.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(s::title_text(
                        if data.credits.is_some_and(|b| b.funding == "hosted-models") {
                            "Hosted model credits".to_owned()
                        } else {
                            tr(cx, msg::settings::prepaid_title)
                        },
                    ))
                    .child(
                        div()
                            .rounded(px(7.))
                            .bg(s::frame_bg())
                            .px(px(8.))
                            .py(px(4.))
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(palette::rgb(TEXT))
                            .child(SharedString::from(
                                data.credits
                                    .map(|b| format!("{} available", cents(b.credit_cents)))
                                    .unwrap_or_else(|| "Balance unavailable".into()),
                            )),
                    ),
            )
            .child(
                button(
                    "refresh-prepaid-credits",
                    "Refresh balance",
                    false,
                    busy,
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !busy {
                        act(this, Action::RefreshCredits, window, cx)
                    }
                })),
            ),
    );
    if let Some(balance) = data.credits.filter(|b| b.debit_cents > 0) {
        credits = credits.child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette::rgb(MUTED))
                .child(format!(
                    "{} owed on future invoices",
                    cents(balance.debit_cents)
                )),
        );
    }
    if let Some(credit) = data.credits.and_then(|b| b.invoice_credit_cents) {
        credits = credits.child(
            div()
                .text_size(px(12.))
                .text_color(palette::rgb(MUTED))
                .child(format!(
                    "{} in compute invoice credit, separate from hosted model credits.",
                    cents(credit)
                )),
        );
    }
    let mut options = div()
        .flex()
        .flex_wrap()
        .gap(px(6.))
        .id("cloud-credit-amounts")
        .role(Role::RadioGroup)
        .aria_label(tr(cx, msg::settings::prepaid_amount));
    for (i, amount) in [1000, 2500, 5000, 10000].into_iter().enumerate() {
        let handles = focus.clone();
        let selected = data.credit_amount == amount;
        let label = cents(amount);
        let mut fill = s::frame_bg().opacity(0.5);
        if selected {
            fill = palette::tone(
                hsla(130. / 360., 0.06, 0.165, 1.),
                hsla(106. / 360., 0.18, 0.89, 1.),
            );
        }
        options = options.child(
            div()
                .id(("credit-amount", i))
                .role(Role::RadioButton)
                .aria_label(SharedString::from(label.clone()))
                .aria_selected(selected)
                .track_focus(&focus[i].clone().tab_stop(enabled && selected))
                .tab_index(if enabled && selected { 0 } else { -1 })
                .h(px(28.))
                .px(px(12.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .bg(fill)
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette::rgb(TEXT))
                .when(selected, |v| {
                    v.font_weight(FontWeight::MEDIUM)
                        .border_1()
                        .border_color(palette::rgb(TEXT).opacity(0.2))
                        .px(px(11.))
                })
                .when(!enabled, |v| v.opacity(0.5))
                .when(enabled, |v| {
                    v.cursor_pointer().hover(|v| v.bg(s::frame_bg()))
                })
                .focus_visible(|v| v.border_2().border_color(palette::rgb(0x89ca21)))
                .child(SharedString::from(label))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if !enabled {
                        return;
                    }
                    let next = match event.keystroke.key.as_str() {
                        "left" | "up" => (i + 3) % 4,
                        "right" | "down" => (i + 1) % 4,
                        "home" => 0,
                        "end" => 3,
                        "enter" | "space" => {
                            cx.stop_propagation();
                            return;
                        }
                        _ => return,
                    };
                    act(
                        this,
                        Action::SelectCredit([1000, 2500, 5000, 10000][next]),
                        window,
                        cx,
                    );
                    window.focus(&handles[next], cx);
                    cx.stop_propagation();
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if enabled {
                        act(this, Action::SelectCredit(amount), window, cx)
                    }
                })),
        );
    }
    credits = credits.child(
        div()
            .flex()
            .flex_wrap()
            .items_end()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .mb(px(6.))
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(palette::rgb(MUTED))
                            .child(tr(cx, msg::settings::prepaid_amount)),
                    )
                    .child(options),
            )
            .child(
                button("buy-prepaid-credits", "Buy credits", true, !enabled, busy).on_click(
                    cx.listener(move |this, _, window, cx| {
                        if enabled {
                            act(this, Action::BuyCredits, window, cx)
                        }
                    }),
                ),
            ),
    );
    credits = credits.child(
        div()
            .text_size(px(12.))
            .line_height(px(19.5))
            .text_color(palette::rgb(MUTED))
            .child(if data.credits.is_some_and(|b|b.funding=="hosted-models") { "For models billed by Zuse. These credits do not pay your Cloud compute invoice or your own agent subscriptions.".to_owned() } else { tr(cx, msg::settings::prepaid_description) }),
    );
    if data.credits.is_some_and(|b| !b.available) {
        credits = credits.child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette::rgb(MUTED))
                .child(tr(cx, msg::settings::prepaid_disabled)),
        );
    }
    if let Some(error) = data.credit_error {
        credits = credits.child(note(error, true));
    }
    credits
}
