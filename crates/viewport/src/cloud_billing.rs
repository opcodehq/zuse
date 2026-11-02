//! Account-scoped Cloud usage controls; requests always use the host authority.
use super::*;
use zuse_model::cloud_billing::{cap_dollars, cap_micros};
use zuse_protocol::cloud_billing::{BillingSummary, BillingUsageItem, BillingUsagePage};
#[derive(Default)]
pub(super) struct Billing {
    pub(super) tab: crate::cloud_settings::Tab,
    owner: Option<(String, Option<String>)>,
    requests: HashMap<u64, (&'static str, Option<(String, Option<String>)>)>,
    subscribed: Option<bool>,
    credits: Option<zuse_protocol::cloud_billing::PrepaidBalance>,
    credit_amount: Option<i64>,
    credit_error: Option<String>,
    summary: Option<BillingSummary>,
    items: Vec<BillingUsageItem>,
    cursor: Option<String>,
    workspaces: Vec<Value>,
    history_open: bool,
    error: Option<String>,
    notice: Option<String>,
    cap_request: Option<(i64, String)>,
}
impl WorkspaceView {
    fn call_billing(&mut self, method: &str, mut params: Value) -> u64 {
        if let Some((account, organization)) = &self.billing.owner {
            params["expectedAccountId"] = json!(account);
            params["expectedOrganizationId"] = json!(organization);
        }
        self.call(method, params)
    }
    pub(super) fn refresh_billing(&mut self, more: bool) {
        if self.billing.owner.is_none() || !self.billing.requests.is_empty() {
            return;
        }
        self.billing.error = None;
        self.billing.notice = None;
        if !more {
            let credit_id = self.call_billing(methods::CLOUD_PREPAID_BALANCE, json!({}));
            self.billing
                .requests
                .insert(credit_id, ("credits", self.billing.owner.clone()));
            let id = self.call_billing(methods::CLOUD_BILLING_ENTITLEMENTS, json!({}));
            self.billing
                .requests
                .insert(id, ("entitlements", self.billing.owner.clone()));
        }
        if !more
            && self
                .billing
                .owner
                .as_ref()
                .is_some_and(|(_, org)| org.is_none())
        {
            let id = self.call_billing(methods::CLOUD_WORKSPACES, json!({}));
            self.billing
                .requests
                .insert(id, ("workspaces", self.billing.owner.clone()));
        }
        let summary = self.call_billing(methods::CLOUD_BILLING_SUMMARY, json!({}));
        self.billing
            .requests
            .insert(summary, ("summary", self.billing.owner.clone()));
        let cursor = if more {
            self.billing.cursor.clone()
        } else {
            None
        };
        let usage = self.call_billing(
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
    fn open_billing_portal(&mut self, force_portal: bool, cx: &mut Context<Self>) {
        if self.billing.owner.is_none()
            || self
                .billing
                .requests
                .values()
                .any(|(kind, _)| *kind == "portal")
        {
            return;
        }
        let subscribed = if force_portal {
            true
        } else if let Some(value) = self.billing.subscribed {
            value
        } else {
            return;
        };
        let id = self.call_billing(
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
        );
        self.billing
            .requests
            .insert(id, ("portal", self.billing.owner.clone()));
        self.billing.error = None;
        cx.notify();
    }
    fn refresh_credits(&mut self, buy: bool, cx: &mut Context<Self>) {
        if self.billing.owner.is_none()
            || self
                .billing
                .requests
                .values()
                .any(|(kind, _)| kind.starts_with("credit"))
        {
            return;
        }
        if buy && !self.billing.credits.as_ref().is_some_and(|v| v.available) {
            return;
        }
        let id = self.call_billing(
            if buy {
                methods::CLOUD_PREPAID_CHECKOUT
            } else {
                methods::CLOUD_PREPAID_BALANCE
            },
            if buy {
                json!({"amountCents":self.billing.credit_amount.unwrap_or(2500)})
            } else {
                json!({})
            },
        );
        self.billing.requests.insert(
            id,
            (
                if buy { "credit-checkout" } else { "credits" },
                self.billing.owner.clone(),
            ),
        );
        self.billing.credit_error = None;
        cx.notify();
    }
    fn save_billing_cap(&mut self, cx: &mut Context<Self>) {
        if self.billing.owner.is_none() || !self.billing.requests.is_empty() {
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
        let id = self.call_billing(
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
            .is_some_and(|m| m == methods::CLOUD_ACCOUNT_STATUS)
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
            if kind.starts_with("credit") {
                self.billing.credit_error = Some(error.into());
                cx.notify();
                return true;
            }
            if self.billing.subscribed != Some(false)
                || error != "An active cloud billing period is required."
            {
                self.billing.error = Some(error.into());
            }
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
        if kind == "workspaces" {
            self.billing.workspaces = value["workspaces"].as_array().cloned().unwrap_or_default();
        } else if kind == "credits" {
            match serde_json::from_value(value["data"].clone()) {
                Ok(balance) => self.billing.credits = Some(balance),
                Err(_) => {
                    self.billing.credit_error =
                        Some("Could not read the prepaid balance. Refresh to retry.".into())
                }
            }
        } else if kind == "credit-checkout" {
            if let Some(url) = value["data"]["checkoutUrl"]
                .as_str()
                .filter(|v| v.starts_with("https://"))
            {
                cx.open_url(url);
            } else {
                self.billing.credit_error =
                    Some("The provider returned an invalid credit checkout link.".into());
            }
        } else if kind == "entitlements" {
            self.billing.subscribed = zuse_model::cloud_billing::cloud_subscribed(
                &value["data"],
                chrono::Utc::now().timestamp_millis(),
            );
            if self.billing.subscribed == Some(false)
                && self.billing.error.as_deref()
                    == Some("An active cloud billing period is required.")
            {
                self.billing.error = None;
            }
            if self.billing.subscribed.is_none() {
                self.billing.error =
                    Some("Could not read the subscription. Refresh to retry.".into());
            }
        } else if kind == "portal" {
            if let Some(url) = value["data"]["portalUrl"]
                .as_str()
                .or_else(|| value["data"]["checkoutUrl"].as_str())
                .map(str::to_owned)
                .as_deref()
                .filter(|url| url.starts_with("https://"))
            {
                cx.open_url(url);
            } else {
                self.billing.error =
                    Some("The billing provider returned an invalid portal link.".into());
            }
        } else if matches!(kind, "summary" | "cap") {
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
        if self.billing.owner.is_none() {
            if self.cloud_account.is_null() && self.account_action_error.is_none() {
                return div().into_any_element();
            }
            let pending = self
                .pending
                .values()
                .any(|method| method == methods::CLOUD_ACCOUNT_SIGN_IN);
            let login = self.cloud_account["login"]["url"]
                .as_str()
                .map(str::to_owned);
            return crate::cloud_settings::account_gate(
                crate::cloud_settings::sign_in_button(
                    "cloud-account-sign-in",
                    if pending {
                        "Signing in…"
                    } else if login.is_some() {
                        "Continue sign-in"
                    } else {
                        "Sign in"
                    },
                )
                .when(pending, |view| view.opacity(0.6))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(url) = login.as_ref().filter(|url| url.starts_with("https://")) {
                        cx.open_url(url);
                    } else if !pending {
                        this.account_card
                            .update(cx, |_, cx| cx.emit(crate::account_settings::Event::SignIn));
                    }
                })),
                self.account_action_error
                    .as_deref()
                    .or_else(|| self.cloud_account["error"].as_str()),
            )
            .into_any_element();
        }
        use crate::cloud_billing_view as view;
        let pending = |kind| self.billing.requests.values().any(|(tag, _)| *tag == kind);
        let data = view::Data {
            subscribed: self.billing.subscribed,
            pending: view::Pending {
                loading: pending("summary")
                    || pending("usage")
                    || pending("more")
                    || pending("entitlements"),
                subscription: pending("portal"),
                credits: pending("credit-checkout") || pending("credits"),
                cap: pending("cap"),
            },
            summary: self.billing.summary.as_ref(),
            usage: &self.billing.items,
            credits: self.billing.credits.as_ref(),
            credit_amount: self.billing.credit_amount.unwrap_or(2500),
            error: self.billing.error.as_deref(),
            credit_error: self.billing.credit_error.as_deref(),
            more: self.billing.cursor.is_some(),
            history_open: self.billing.history_open,
            workspaces: &self.billing.workspaces,
            personal: self
                .billing
                .owner
                .as_ref()
                .is_some_and(|(_, org)| org.is_none()),
        };
        let action = |this: &mut Self, action, _window: &mut Window, cx: &mut Context<Self>| {
            match action {
                view::Action::Refresh => this.refresh_billing(false),
                view::Action::RefreshCredits => this.refresh_credits(false, cx),
                view::Action::Subscription => this.open_billing_portal(false, cx),
                view::Action::ManageSubscription => this.open_billing_portal(true, cx),
                view::Action::SaveCap => this.save_billing_cap(cx),
                view::Action::SelectCredit(amount) => this.billing.credit_amount = Some(amount),
                view::Action::BuyCredits => this.refresh_credits(true, cx),
                view::Action::MoreUsage => this.refresh_billing(true),
                view::Action::ToggleUsageHistory => {
                    this.billing.history_open = !this.billing.history_open
                }
            }
            cx.notify();
        };
        let mut body = div().flex().flex_col().gap(px(16.)).child(view::hosting(
            &data,
            if data.personal {
                "Personal workspace"
            } else {
                "Organization workspace"
            },
            cx,
            action,
        ));
        if self.billing.subscribed == Some(true) {
            body = body.child(crate::cloud_settings::tabs(
                self.billing.tab,
                &self.cloud_settings_focus,
                cx,
                |this: &mut Self, tab, _, cx| {
                    this.billing.tab = tab;
                    if tab == crate::cloud_settings::Tab::Usage {
                        this.refresh_billing(false);
                    }
                    cx.notify();
                },
            ));
        }
        let storage = self.repository_panel.update(cx, |panel, cx| {
            panel.set_usage(
                self.billing.tab == crate::cloud_settings::Tab::Usage,
                self.billing.summary.as_ref().map(|s| s.storage_cost_micros),
                cx,
            )
        });
        if self.billing.tab == crate::cloud_settings::Tab::Setup {
            if self.billing.subscribed == Some(true) {
                body = body.child(self.repository_panel.clone());
            }
        } else {
            if storage {
                body = body.child(self.repository_panel.clone());
            }
            body = body.child(view::usage(
                &data,
                self.billing_cap.clone().into_any_element(),
                &self.cloud_credit_focus,
                cx,
                action,
            ));
        }
        body.into_any_element()
    }
}
