//! Existing cloud billing wire contracts. Amounts are USD millionths, never floats.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BillingStatus {
    Active,
    Grace,
    BillingHold,
    Ended,
    Manual,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BillingSummary {
    pub currency: String,
    pub status: BillingStatus,
    pub period_start: i64,
    pub period_end: i64,
    pub base_price_micros: i64,
    pub included_provider_cost_micros: i64,
    pub provider_cost_micros: i64,
    pub storage_cost_micros: i64,
    pub included_used_micros: i64,
    pub included_remaining_micros: i64,
    pub overage_provider_cost_micros: i64,
    pub overage_charge_micros: i64,
    pub overage_cap_micros: i64,
    pub markup_basis_points: i64,
    pub current_invoice_estimate_micros: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_provider_reconciled_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_billing_reconciled_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_polar_reconciled_at: Option<i64>,
    pub usage_provisional: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BillingUsageItem {
    pub entry_id: String,
    pub resource_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_kind: Option<String>,
    pub resource_id: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_execution_id: Option<String>,
    pub started_at: i64,
    pub ended_at: i64,
    pub vcpu_count: i64,
    pub memory_mib: i64,
    pub provider_cost_micros: i64,
    pub status: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BillingUsagePage {
    pub items: Vec<BillingUsageItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingCapRequest {
    pub overage_cap_micros: i64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BillingUsageRequest {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingCheckoutRequest {
    pub offer_id: String,
}

pub const PREPAID_CREDIT_AMOUNTS: [i64; 4] = [1000, 2500, 5000, 10000];
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepaidBalance {
    pub available: bool,
    pub credit_cents: i64,
    pub debit_cents: i64,
    pub currency: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepaidCheckoutRequest {
    pub amount_cents: i64,
}
