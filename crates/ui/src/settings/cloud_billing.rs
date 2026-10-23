//! Reference Cloud → Usage and billing, backed by the host account authority.
use crate::{composer::ComposerInput, popover, settings::widgets, state::AppState, theme::Theme};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use zuse_model::cloud_billing::{cap_dollars, cap_micros, usd};
use zuse_protocol::{
    AuthState,
    cloud_billing::{BillingSummary, BillingUsageItem, BillingUsagePage},
};
use zuse_rpc::methods;
type Owner = (String, Option<String>);
fn active_owner(state: &AppState) -> Option<Owner> {
    match state.auth.as_ref()? {
        AuthState::SignedIn { user, org_id } => Some((user.id.clone(), org_id.clone())),
        _ => None,
    }
}
fn bound(value: &Value, owner: &Owner) -> bool {
    value["accountId"].as_str() == Some(owner.0.as_str())
        && value["organizationId"].as_str() == owner.1.as_deref()
}
pub struct CloudBillingPage {
    state: Entity<AppState>,
    owner: Option<Owner>,
    loaded: bool,
    task: Option<Task<()>>,
    portal_task: Option<Task<()>>,
    subscription_task: Option<Task<()>>,
    subscribed: Option<bool>,
    credits: Option<zuse_protocol::cloud_billing::PrepaidBalance>,
    credit_task: Option<Task<()>>,
    credit_amount: i64,
    credit_error: Option<String>,
    summary: Option<BillingSummary>,
    items: Vec<BillingUsageItem>,
    cursor: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    cap: Entity<ComposerInput>,
    cap_request: Option<(i64, String)>,
}
impl CloudBillingPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let cap = cx
            .new(|cx| ComposerInput::new("Monthly overage cap in dollars", cx).with_single_line());
        cx.observe(&state, |_: &mut Self, _, cx| cx.notify())
            .detach();
        Self {
            state,
            owner: None,
            loaded: false,
            task: None,
            portal_task: None,
            subscription_task: None,
            subscribed: None,
            credits: None,
            credit_task: None,
            credit_amount: 2500,
            credit_error: None,
            summary: None,
            items: vec![],
            cursor: None,
            error: None,
            notice: None,
            cap,
            cap_request: None,
        }
    }
    fn load(&mut self, more: bool, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        if !more {
            self.load_subscriptions(cx);
            self.load_credits(false, cx);
        }
        self.loaded = true;
        self.error = None;
        let cursor = if more { self.cursor.clone() } else { None };
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            futures::try_join!(
                engine
                    .client()
                    .call_scoped(methods::CLOUD_BILLING_SUMMARY, json!({})),
                engine.client().call_scoped(
                    methods::CLOUD_BILLING_USAGE,
                    json!({"limit":10,"cursor":cursor})
                )
            )
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.task = None;
                match result {
                    Ok(Ok((summary, page))) if bound(&summary, &owner) && bound(&page, &owner) => {
                        let data =
                            serde_json::from_value::<BillingSummary>(summary["data"].clone())
                                .and_then(|summary| {
                                    serde_json::from_value::<BillingUsagePage>(page["data"].clone())
                                        .map(|page| (summary, page))
                                });
                        match data {
                            Ok((summary, page)) => {
                                if !more {
                                    this.items.clear();
                                    this.cap.update(cx, |input, cx| {
                                        input.set_text(cap_dollars(summary.overage_cap_micros), cx)
                                    });
                                }
                                for item in page.items {
                                    if !this.items.iter().any(|old| old.entry_id == item.entry_id) {
                                        this.items.push(item);
                                    }
                                }
                                this.cursor = page.next_cursor;
                                this.summary = Some(summary);
                            }
                            Err(_) => {
                                this.error = Some(
                                    "Cloud billing returned an invalid response. Refresh to retry."
                                        .into(),
                                )
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        let message = error.to_string();
                        if this.subscribed != Some(false)
                            || message != "An active cloud billing period is required."
                        {
                            this.error = Some(message);
                        }
                    }
                    _ => {
                        this.error = Some("Could not load cloud billing. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
    }
    fn load_subscriptions(&mut self, cx: &mut Context<Self>) {
        if self.subscription_task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(methods::CLOUD_BILLING_ENTITLEMENTS, json!({}))
                .await
        });
        self.subscription_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.subscription_task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        this.subscribed = zuse_model::cloud_billing::cloud_subscribed(
                            &value["data"],
                            chrono::Utc::now().timestamp_millis(),
                        );
                        if this.subscribed == Some(false)
                            && this.error.as_deref()
                                == Some("An active cloud billing period is required.")
                        {
                            this.error = None;
                        }
                        if this.subscribed.is_none() {
                            this.error =
                                Some("Could not read the subscription. Refresh to retry.".into());
                        }
                    }
                    Ok(Err(e)) => this.error = Some(e.to_string()),
                    _ => {
                        this.error =
                            Some("Could not load the subscription. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
    }
    fn billing_link(&mut self, cx: &mut Context<Self>) {
        if self.portal_task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        let Some(subscribed) = self.subscribed else {
            return;
        };
        self.error = None;
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    if subscribed {
                        methods::CLOUD_BILLING_PORTAL
                    } else {
                        methods::CLOUD_BILLING_CHECKOUT
                    },
                    if subscribed {
                        json!({})
                    } else {
                        json!({"offerId":"cloud-workspace-standard-v1"})
                    },
                )
                .await
        });
        self.portal_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.portal_task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        if let Some(url) = value["data"][if subscribed {
                            "portalUrl"
                        } else {
                            "checkoutUrl"
                        }]
                        .as_str()
                        .filter(|url| url.starts_with("https://"))
                        {
                            cx.open_url(url);
                        } else {
                            this.error = Some(
                                "The billing provider returned an invalid portal link.".into(),
                            );
                        }
                    }
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    _ => this.error = Some("Could not open billing. Try again.".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn load_credits(&mut self, buy: bool, cx: &mut Context<Self>) {
        if self.credit_task.is_some() {
            return;
        }
        if buy && !self.credits.as_ref().is_some_and(|v| v.available) {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        let amount = self.credit_amount;
        self.credit_error = None;
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    if buy {
                        methods::CLOUD_PREPAID_CHECKOUT
                    } else {
                        methods::CLOUD_PREPAID_BALANCE
                    },
                    if buy {
                        json!({"amountCents":amount})
                    } else {
                        json!({})
                    },
                )
                .await
        });
        self.credit_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.credit_task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        if buy {
                            if let Some(url) = value["data"]["checkoutUrl"]
                                .as_str()
                                .filter(|s| s.starts_with("https://"))
                            {
                                cx.open_url(url);
                            } else {
                                this.credit_error = Some(
                                    "The provider returned an invalid credit checkout link.".into(),
                                );
                            }
                        } else {
                            match serde_json::from_value(value["data"].clone()) {
                                Ok(balance) => this.credits = Some(balance),
                                Err(_) => {
                                    this.credit_error = Some(
                                        "Could not read the prepaid balance. Refresh to retry."
                                            .into(),
                                    )
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => this.credit_error = Some(e.to_string()),
                    _ => {
                        this.credit_error =
                            Some("Could not load prepaid credits. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let Some(amount) = cap_micros(self.cap.read(cx).text()) else {
            self.error =
                Some("Enter a nonnegative dollar amount with up to six decimal places.".into());
            cx.notify();
            return;
        };
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        if !self
            .cap_request
            .as_ref()
            .is_some_and(|(value, _)| *value == amount)
        {
            self.cap_request = Some((amount, uuid::Uuid::new_v4().to_string()));
        }
        let key = self.cap_request.as_ref().unwrap().1.clone();
        self.error = None;
        self.notice = None;
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(
                    methods::CLOUD_BILLING_CAP,
                    json!({"overageCapMicros":amount,"idempotencyKey":key}),
                )
                .await
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if active_owner(this.state.read(cx)).as_ref() != Some(&owner) {
                    return;
                }
                this.task = None;
                match result {
                    Ok(Ok(value)) if bound(&value, &owner) => {
                        match serde_json::from_value::<BillingSummary>(value["data"].clone()) {
                            Ok(summary) => {
                                this.cap.update(cx, |input, cx| {
                                    input.set_text(cap_dollars(summary.overage_cap_micros), cx)
                                });
                                this.summary = Some(summary);
                                this.cap_request = None;
                                this.notice = Some("Monthly overage cap saved.".into());
                            }
                            Err(_) => this.error = Some(
                                "Could not verify the saved cap. Refresh before changing it again."
                                    .into(),
                            ),
                        }
                    }
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    _ => {
                        this.error = Some(
                            "The cap request was interrupted. Retry to check the same request."
                                .into(),
                        )
                    }
                }
                cx.notify();
            });
        }));
    }
}
impl Render for CloudBillingPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = active_owner(self.state.read(cx));
        if self.owner != active {
            self.owner = active;
            self.loaded = false;
            self.task = None;
            self.portal_task = None;
            self.subscription_task = None;
            self.subscribed = None;
            self.credits = None;
            self.credit_task = None;
            self.credit_amount = 2500;
            self.credit_error = None;
            self.summary = None;
            self.items.clear();
            self.cursor = None;
            self.error = None;
            self.notice = None;
            self.cap_request = None;
            self.cap.update(cx, |i, cx| i.set_text("", cx));
        }
        if !self.loaded {
            self.load(false, cx);
        }
        let theme = Theme::of(cx).for_settings_surface();
        let busy = self.task.is_some();
        let mut body = widgets::page_column().child(widgets::section_title_with_action(
            &theme,
            "Cloud",
            None,
            "Usage and billing for the selected account.",
            Some(
                widgets::action_button(&theme, widgets::ActionTone::Quiet)
                    .child(if busy { "Loading…" } else { "Refresh" })
                    .id("refresh-cloud-billing")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        this.load(false, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
        ));
        if self.owner.is_some() {
            body = body.child(widgets::group(&theme, "Subscription", if self.subscribed==Some(false){"$40 per month includes $35 of compute. Additional compute has a 5% markup."}else{"Manage your existing subscription, invoices and payment method with your billing provider."})
                .child(widgets::card_row(&theme, false)
                    .child(div().flex_1().child(widgets::row_title(&theme, if self.subscribed==Some(false){"Cloud subscription"}else{"Billing portal"})))
                    .child(widgets::action_button(&theme, widgets::ActionTone::Quiet)
                        .id("open-cloud-billing-portal")
                        .child(if self.portal_task.is_some() { "Opening…" } else if self.subscribed==Some(false){"Subscribe to Cloud"}else if self.subscribed.is_none(){"Loading…"}else{ "Manage billing" })
                        .on_click(cx.listener(|this, _, _, cx| this.billing_link(cx))))));
        }
        if self.owner.is_some() {
            let enabled =
                self.credits.as_ref().is_some_and(|v| v.available) && self.credit_task.is_none();
            let mut amounts = div().flex().gap(px(6.)).flex_1();
            for amount in zuse_protocol::cloud_billing::PREPAID_CREDIT_AMOUNTS {
                amounts = amounts.child(
                    widgets::action_button(
                        &theme,
                        if self.credit_amount == amount {
                            widgets::ActionTone::Solid
                        } else {
                            widgets::ActionTone::Quiet
                        },
                    )
                    .id(("credit-amount", amount as usize))
                    .role(Role::RadioButton)
                    .aria_selected(self.credit_amount == amount)
                    .child(SharedString::from(zuse_model::cloud_billing::cents(amount)))
                    .when(!enabled, |view| view.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.credit_amount = amount;
                            cx.notify();
                        }
                    })),
                );
            }
            let balance = self
                .credits
                .as_ref()
                .map(|v| {
                    format!(
                        "Balance: {}",
                        zuse_model::cloud_billing::cents(v.credit_cents)
                    )
                })
                .unwrap_or_else(|| "Loading balance…".into());
            let mut credits=widgets::group(&theme,"Prepaid credits","Applies automatically to future Zuse invoices, including subscriptions. Unused credit rolls forward. Your usage allowance and spending cap stay the same.")
                .child(widgets::card_row(&theme,false).child(div().flex_1().child(widgets::row_title(&theme,balance)))
                    .child(widgets::action_button(&theme,widgets::ActionTone::Quiet).id("refresh-prepaid-credits").child("Refresh").on_click(cx.listener(|this,_,_,cx|this.load_credits(false,cx)))))
                .child(widgets::card_row(&theme,false).child(amounts)
                    .child(widgets::action_button(&theme,widgets::ActionTone::Solid).id("buy-prepaid-credits").child(if self.credit_task.is_some(){"Loading…"}else{"Buy credits"}).when(!enabled,|view|view.opacity(0.5)).on_click(cx.listener(|this,_,_,cx|this.load_credits(true,cx)))));
            if let Some(balance) = &self.credits {
                if balance.debit_cents > 0 {
                    credits =
                        credits.child(widgets::card_row(&theme, false).child(widgets::row_title(
                            &theme,
                            format!(
                                "Outstanding balance: {}",
                                zuse_model::cloud_billing::cents(balance.debit_cents)
                            ),
                        )));
                }
                if !balance.available {
                    credits =
                        credits.child(widgets::card_row(&theme, false).child(widgets::row_title(
                            &theme,
                            "Credit purchases are not enabled for this workspace yet.",
                        )));
                }
            }
            if let Some(error) = &self.credit_error {
                credits = credits.child(
                    widgets::card_row(&theme, false)
                        .child(widgets::row_title(&theme, error.clone())),
                );
            }
            body = body.child(credits);
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(error.clone())),
            );
        }
        if let Some(notice) = &self.notice {
            body = body.child(
                div()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(notice.clone())),
            );
        }
        if let Some(summary) = &self.summary {
            let status = match summary.status {
                zuse_protocol::cloud_billing::BillingStatus::BillingHold => "Billing hold",
                zuse_protocol::cloud_billing::BillingStatus::Ended => "Ended",
                zuse_protocol::cloud_billing::BillingStatus::Grace => "Grace",
                zuse_protocol::cloud_billing::BillingStatus::Manual => "Manual",
                _ => "Active",
            };
            let description = format!(
                "{} per month includes {} of compute. Additional compute markup: {}%.",
                usd(summary.base_price_micros),
                usd(summary.included_provider_cost_micros),
                summary.markup_basis_points as f64 / 100.
            );
            let row = |title: String, description: String| {
                widgets::card_row(&theme, false).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(&theme, title))
                        .child(
                            div()
                                .text_color(theme.text_muted)
                                .text_size(px(12.))
                                .child(description),
                        ),
                )
            };
            body=body.child(widgets::group(&theme,"Usage and billing",description)
                .child(row(format!("{} of {} included used",usd(summary.included_used_micros),usd(summary.included_provider_cost_micros)),format!("{} remaining · {} overage · {} invoice estimate before tax",usd(summary.included_remaining_micros),usd(summary.overage_charge_micros),usd(summary.current_invoice_estimate_micros))))
                .child(row(status.into(),if summary.usage_provisional{"Usage includes provisional reservations and may change after reconciliation.".into()}else{"Usage reconciled with the provider.".into()}))
                .child(widgets::card_row(&theme,false).child(div().flex_1().child(widgets::row_title(&theme,"Monthly overage cap")).child(div().text_color(theme.text_muted).text_size(px(12.)).child("New billable operations are blocked when this pre-tax limit is reached.")))
                    .child(div().w(px(110.)).child(popover::dialog_field(self.cap.clone().into_any_element())))
                    .child(widgets::action_button(&theme, widgets::ActionTone::Solid).child(if busy{"Saving…"}else{"Save"}).id("save-cloud-cap").on_click(cx.listener(|this,_,_,cx|this.save(cx))))));
            let mut usage = widgets::group(
                &theme,
                "Recent usage",
                format!(
                    "Retained image storage: {}",
                    usd(summary.storage_cost_micros)
                ),
            );
            for item in &self.items {
                usage = usage.child(row(
                    format!(
                        "{} {}",
                        item.usage_kind.as_deref().unwrap_or(&item.resource_kind),
                        item.resource_id
                    ),
                    format!(
                        "{} · {} · {}",
                        item.provider,
                        usd(item.provider_cost_micros),
                        item.status
                    ),
                ));
            }
            if self.items.is_empty() {
                usage = usage.child(row(
                    "No completed sandbox runs in this billing period.".into(),
                    String::new(),
                ));
            }
            if self.cursor.is_some() {
                usage = usage.child(
                    widgets::action_button(&theme, widgets::ActionTone::Quiet)
                        .child("Load more")
                        .id("more-cloud-usage")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load(true, cx);
                            cx.notify();
                        })),
                );
            }
            body = body.child(usage);
        } else if self.owner.is_none() {
            body = body.child(widgets::row_title(&theme, "Sign in to view cloud billing."));
        }
        div()
            .id("cloud-billing-settings")
            .size_full()
            .overflow_y_scroll()
            .child(body)
    }
}
