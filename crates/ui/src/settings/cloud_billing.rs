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
                    Ok(Err(error)) => this.error = Some(error.to_string()),
                    _ => {
                        this.error = Some("Could not load cloud billing. Refresh to retry.".into())
                    }
                }
                cx.notify();
            });
        }));
    }
    fn portal(&mut self, cx: &mut Context<Self>) {
        if self.portal_task.is_some() {
            return;
        }
        let Some(owner) = active_owner(self.state.read(cx)) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        self.error = None;
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(methods::CLOUD_BILLING_PORTAL, json!({}))
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
                        if let Some(url) = value["data"]["portalUrl"]
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
            body = body.child(widgets::group(&theme, "Subscription", "Manage your existing subscription, invoices and payment method with your billing provider.")
                .child(widgets::card_row(&theme, false)
                    .child(div().flex_1().child(widgets::row_title(&theme, "Billing portal")))
                    .child(widgets::action_button(&theme, widgets::ActionTone::Quiet)
                        .id("open-cloud-billing-portal")
                        .child(if self.portal_task.is_some() { "Opening…" } else { "Manage billing" })
                        .on_click(cx.listener(|this, _, _, cx| this.portal(cx))))));
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
