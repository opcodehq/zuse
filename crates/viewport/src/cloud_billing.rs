//! Account-scoped Cloud usage controls; requests always use the host authority.
use super::*;
use crate::{
    settings_style as s,
    shell::{column, label, row},
};
use zuse_model::cloud_billing::{cap_dollars, cap_micros, usd};
use zuse_protocol::cloud_billing::{BillingSummary, BillingUsageItem, BillingUsagePage};
#[derive(Default)]
pub(super) struct Billing {
    owner: Option<(String, Option<String>)>,
    requests: HashMap<u64, (&'static str, Option<(String, Option<String>)>)>,
    summary: Option<BillingSummary>,
    items: Vec<BillingUsageItem>,
    cursor: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    cap_request: Option<(i64, String)>,
}
impl WorkspaceView {
    pub(super) fn refresh_billing(&mut self, more: bool) {
        if !self.billing.requests.is_empty() {
            return;
        }
        self.billing.error = None;
        self.billing.notice = None;
        let summary = self.call(methods::CLOUD_BILLING_SUMMARY, json!({}));
        self.billing
            .requests
            .insert(summary, ("summary", self.billing.owner.clone()));
        let cursor = if more {
            self.billing.cursor.clone()
        } else {
            None
        };
        let usage = self.call(
            methods::CLOUD_BILLING_USAGE,
            json!({"cursor":cursor,"limit":10}),
        );
        self.billing.requests.insert(
            usage,
            (
                if more { "more" } else { "usage" },
                self.billing.owner.clone(),
            ),
        );
    }
    fn save_billing_cap(&mut self, cx: &mut Context<Self>) {
        if !self.billing.requests.is_empty() {
            return;
        }
        let Some(amount) = cap_micros(&self.billing_cap.read(cx).value(cx)) else {
            self.billing.error = Some("Enter a valid nonnegative dollar amount.".into());
            cx.notify();
            return;
        };
        if !self
            .billing
            .cap_request
            .as_ref()
            .is_some_and(|(value, _)| *value == amount)
        {
            self.billing.cap_request = Some((amount, uuid::Uuid::new_v4().to_string()));
        }
        let key = self.billing.cap_request.as_ref().unwrap().1.clone();
        let id = self.call(
            methods::CLOUD_BILLING_CAP,
            json!({"overageCapMicros":amount,"idempotencyKey":key}),
        );
        self.billing
            .requests
            .insert(id, ("cap", self.billing.owner.clone()));
        self.billing.error = None;
        self.billing.notice = None;
        cx.notify();
    }
    pub(super) fn receive_billing(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(id) = frame["id"].as_u64() else {
            return false;
        };
        if self
            .pending
            .get(&id)
            .is_some_and(|m| m == methods::AUTH_STATUS)
        {
            if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
                let owner = value["user"]["id"]
                    .as_str()
                    .map(|id| (id.to_owned(), value["orgId"].as_str().map(str::to_owned)));
                if self.billing.owner != owner {
                    for id in self.billing.requests.keys().copied().collect::<Vec<_>>() {
                        self.cancel(id);
                    }
                    self.billing = Billing {
                        owner,
                        ..Default::default()
                    };
                    self.billing_cap
                        .update(cx, |input, cx| input.set_value("".into(), cx));
                    if self.settings_section == crate::settings_view::Section::Cloud
                        && self.billing.owner.is_some()
                    {
                        self.refresh_billing(false);
                    }
                }
            }
            return false;
        }
        let Some((kind, owner)) = self.billing.requests.remove(&id) else {
            return false;
        };
        self.pending.remove(&id);
        if owner != self.billing.owner {
            return true;
        }
        if let Some(error) = frame["err"].as_str() {
            self.billing.error = Some(error.into());
            cx.notify();
            return true;
        }
        let Some(value) = frame.get("ok") else {
            self.billing.error =
                Some("Cloud billing returned an invalid response. Refresh to retry.".into());
            cx.notify();
            return true;
        };
        if !owner.as_ref().is_some_and(|(account, org)| {
            value["accountId"].as_str() == Some(account.as_str())
                && value["organizationId"].as_str() == org.as_deref()
        }) {
            return true;
        }
        if matches!(kind, "summary" | "cap") {
            match serde_json::from_value::<BillingSummary>(value["data"].clone()) {
                Ok(summary) => {
                    self.billing_cap.update(cx, |input, cx| {
                        input.set_value(cap_dollars(summary.overage_cap_micros), cx)
                    });
                    self.billing.summary = Some(summary);
                    if kind == "cap" {
                        self.billing.cap_request = None;
                        self.billing.notice = Some("Monthly overage cap saved.".into());
                    }
                }
                Err(_) => {
                    self.billing.error = Some("Cloud billing returned an invalid summary.".into())
                }
            }
        } else {
            match serde_json::from_value::<BillingUsagePage>(value["data"].clone()) {
                Ok(page) => {
                    if kind == "usage" {
                        self.billing.items.clear();
                    }
                    for item in page.items {
                        if !self
                            .billing
                            .items
                            .iter()
                            .any(|old| old.entry_id == item.entry_id)
                        {
                            self.billing.items.push(item);
                        }
                    }
                    self.billing.cursor = page.next_cursor;
                }
                Err(_) => {
                    self.billing.error =
                        Some("Cloud billing returned invalid usage history.".into())
                }
            }
        }
        cx.notify();
        true
    }
    pub(super) fn billing_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = !self.billing.requests.is_empty();
        let mut view = column().gap_5().child(
            row().justify_end().child(
                s::button(
                    "refresh-cloud-billing",
                    if busy { "Loading…" } else { "Refresh" },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.refresh_billing(false);
                    cx.notify();
                })),
            ),
        );
        if let Some(error) = &self.billing.error {
            view = view.child(label(error.clone()));
        }
        if let Some(notice) = &self.billing.notice {
            view = view.child(label(notice.clone()));
        }
        if let Some(summary) = &self.billing.summary {
            let status = match summary.status {
                zuse_protocol::cloud_billing::BillingStatus::BillingHold => "Billing hold",
                zuse_protocol::cloud_billing::BillingStatus::Ended => "Ended",
                zuse_protocol::cloud_billing::BillingStatus::Manual => "Manual",
                zuse_protocol::cloud_billing::BillingStatus::Grace => "Grace",
                _ => "Active",
            };
            view=view.child(s::frame().child(s::header("Usage and billing",format!("{} per month includes {} of compute. Additional compute markup: {}%.",usd(summary.base_price_micros),usd(summary.included_provider_cost_micros),summary.markup_basis_points as f64/100.)))
                .child(s::card().child(column().gap_3().p_3()
                    .child(label(format!("{} of {} included used",usd(summary.included_used_micros),usd(summary.included_provider_cost_micros))))
                    .child(label(format!("{} remaining · {} overage · {} invoice estimate before tax",usd(summary.included_remaining_micros),usd(summary.overage_charge_micros),usd(summary.current_invoice_estimate_micros))))
                    .child(label(format!("{status}{}",if summary.usage_provisional{" · Includes provisional usage"}else{""})))
                    .child(row().gap_3().child(column().flex_1().child(s::title_text("Monthly overage cap")).child(label("New billable operations are blocked when this pre-tax limit is reached.")))
                        .child(s::field().w(px(100.)).child(self.billing_cap.clone()))
                        .child(s::button("save-cloud-cap",if busy{"Saving…"}else{"Save"}).on_click(cx.listener(|this,_,_,cx|this.save_billing_cap(cx))))))));
            view = view.child(
                s::frame()
                    .child(s::header(
                        "Recent usage",
                        format!(
                            "Retained image storage: {}",
                            usd(summary.storage_cost_micros)
                        ),
                    ))
                    .child(
                        s::card().child(
                            column()
                                .gap_3()
                                .p_3()
                                .when(self.billing.items.is_empty(), |view| {
                                    view.child(label(
                                        "No completed sandbox runs in this billing period.",
                                    ))
                                })
                                .children(self.billing.items.iter().map(|item| {
                                    label(format!(
                                        "{} {}: {} · {} · {}",
                                        item.usage_kind.as_deref().unwrap_or(&item.resource_kind),
                                        item.resource_id,
                                        usd(item.provider_cost_micros),
                                        item.provider,
                                        item.status
                                    ))
                                }))
                                .when(self.billing.cursor.is_some(), |view| {
                                    view.child(s::button("more-cloud-usage", "Load more").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.refresh_billing(true);
                                            cx.notify();
                                        }),
                                    ))
                                }),
                        ),
                    ),
            );
        } else if !busy && self.billing.error.is_none() {
            view = view.child(label("Sign in to view cloud billing."));
        }
        view.into_any_element()
    }
}
