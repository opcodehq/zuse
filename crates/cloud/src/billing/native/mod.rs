//! Embedded billing. Resource accounting and legacy invoice credit remain host-owned.
mod commerce;
mod compute;
mod models;
mod projection;
pub use models::ModelReservation;

use super::BillingControl;
use crate::error::ApiError;
use brokeboy::{
    Billing, BillingOptions,
    core::{
        BillingError,
        models::{DatabaseConnection, Environment},
        schema::Dialect,
    },
    provider::{PaymentProvider, Providers},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::HashSet, sync::Arc};

pub const MIGRATION: &str = concat!(
    include_str!("../../../migrations/0032_brokeboy.sql"),
    "\n",
    include_str!("../../../migrations/0033_shared_credits.sql")
);
pub const CREDIT_WALLET: &str = "zuse-usd";
const CLOUD_PLAN: &str = "cloud-workspace-standard-v1";

pub struct NativeBilling {
    engine: Billing,
    provider: Arc<dyn PaymentProvider>,
    accounts: HashSet<String>,
    default_provider: String,
    return_origin: String,
    automatic_tax: bool,
    stripe_cloud_price: Option<String>,
    pub(in crate::billing) signer: Option<crate::tickets::TicketSigner>,
}

pub(super) fn failure(error: BillingError) -> ApiError {
    // Provider diagnostics can contain customer data; only expose our bounded error vocabulary.
    match error.code.as_str() {
        "invalid_signature" | "environment_mismatch" => {
            ApiError::unauthorized("invalid_billing_event")
        }
        "insufficient_balance" | "spend_limit_exceeded" => {
            ApiError::forbidden("shared_credit_exhausted")
        }
        "customer_mapping_conflict" => {
            ApiError::conflict("billing_customer_reconciliation_required")
        }
        "idempotency_conflict" | "settlement_conflict" => {
            ApiError::conflict("billing_idempotency_conflict")
        }
        "customer_not_found" => ApiError::conflict("billing_enrollment_required"),
        _ => ApiError::unavailable("billing_operation_incomplete"),
    }
}
pub(super) fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::unavailable("invalid_billing_result"))
}
fn env_required(key: &str) -> anyhow::Result<String> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{key} is required for embedded billing"))
}
pub(super) fn secure_url(value: &str) -> Result<(), ApiError> {
    let url =
        reqwest::Url::parse(value).map_err(|_| ApiError::unavailable("invalid_billing_url"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::unavailable("invalid_billing_url"));
    }
    Ok(())
}
impl NativeBilling {
    /// Opens its own SQLx pool. Schema/catalog changes are explicit operator actions.
    pub async fn from_env(database_url: &str) -> anyhow::Result<Option<Self>> {
        let accounts: HashSet<String> = std::env::var("ZUSE_BROKEBOY_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if accounts.is_empty() {
            return Ok(None);
        }
        let environment = match env_required("BROKEBOY_ENVIRONMENT")?.as_str() {
            "test" => Environment::Test,
            "live" => Environment::Live,
            _ => anyhow::bail!("BROKEBOY_ENVIRONMENT must be test or live"),
        };
        let allowed: HashSet<String> = env_required("ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS")?
            .split(',')
            .map(str::trim)
            .map(str::to_owned)
            .collect();
        anyhow::ensure!(
            accounts.is_subset(&allowed),
            "Embedded billing accounts must have billing writer ownership"
        );
        for account in &accounts {
            brokeboy::core::error::identifier(account)
                .map_err(|_| anyhow::anyhow!("Invalid embedded billing account"))?;
        }
        let default_provider =
            std::env::var("BILLING_DEFAULT_PROVIDER").unwrap_or_else(|_| "stripe".into());
        let mut adapters: Vec<Arc<dyn PaymentProvider>> = vec![];
        if let Ok(key) = std::env::var("STRIPE_SECRET_KEY") {
            let mut options =
                brokeboy::stripe::StripeOptions::new(key, env_required("STRIPE_WEBHOOK_SECRET")?);
            options.environment = environment;
            adapters.push(Arc::new(brokeboy::stripe::Stripe::new(options).map_err(
                |_| anyhow::anyhow!("Invalid embedded Stripe configuration"),
            )?));
        }
        if let Ok(key) = std::env::var("POLAR_ACCESS_TOKEN") {
            let mut options =
                brokeboy::polar::PolarOptions::new(key, env_required("POLAR_WEBHOOK_SECRET")?);
            options.environment = environment;
            if let Ok(product) = std::env::var("POLAR_PRODUCT_CLOUD_WORKSPACE_STANDARD_V1") {
                options.products.insert(product, CLOUD_PLAN.into());
            }
            options.legacy_literal_secret =
                std::env::var("POLAR_LEGACY_LITERAL_SECRET").as_deref() == Ok("true");
            adapters.push(Arc::new(brokeboy::polar::Polar::new(options).map_err(
                |_| anyhow::anyhow!("Invalid embedded Polar configuration"),
            )?));
        }
        let provider: Arc<dyn PaymentProvider> = Arc::new(
            Providers::new(default_provider.clone(), adapters)
                .map_err(|_| anyhow::anyhow!("Embedded payment processor unavailable"))?,
        );
        let return_origin = env_required("API_ISSUER")?;
        secure_url(&return_origin).map_err(|_| anyhow::anyhow!("Invalid billing return origin"))?;
        let signer = if std::env::var("MACHINE_LIVE_CHECKOUT_ENABLED").as_deref() == Ok("true") {
            Some(
                crate::tickets::TicketSigner::from_jwk(
                    &env_required("RELAY_MINT_PRIVATE_JWK")?,
                    &return_origin,
                )
                .map_err(|_| anyhow::anyhow!("Invalid checkout signing configuration"))?,
            )
        } else {
            None
        };
        let engine = Billing::open(
            DatabaseConnection {
                driver: Dialect::Postgres,
                url: database_url.into(),
                max_connections: Some(4),
                acquire_timeout_ms: Some(5000),
            },
            BillingOptions {
                environment,
                provider: Some(provider.clone()),
                owns_provider: true,
                ..Default::default()
            },
        )
        .await
        .map_err(|_| anyhow::anyhow!("Cannot open embedded billing database"))?;
        engine
            .call("diagnostics.doctor", json!({}))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Apply migration 0032_brokeboy.sql before enabling embedded billing"
                )
            })?;
        Ok(Some(Self {
            engine,
            provider,
            accounts,
            default_provider,
            return_origin: return_origin.trim_end_matches('/').into(),
            automatic_tax: std::env::var("STRIPE_AUTOMATIC_TAX_ENABLED").as_deref() != Ok("false"),
            signer,
            stripe_cloud_price: std::env::var("STRIPE_PRICE_CLOUD_WORKSPACE_STANDARD_V1").ok(),
        }))
    }
    pub async fn close(&self) -> Result<(), ApiError> {
        self.engine.close().await.map_err(failure)
    }
    pub async fn publish_catalog(
        &self,
        catalog: Value,
        expected_revision: i64,
    ) -> Result<Value, ApiError> {
        if let Some(plan) = catalog["plans"]
            .as_array()
            .and_then(|plans| plans.iter().find(|p| p["id"] == CLOUD_PLAN))
        {
            self.validate_compute_plan(plan)?;
        }
        self.engine
            .call(
                "catalog.publish",
                json!({"catalog":catalog,"expectedRevision":expected_revision}),
            )
            .await
            .map_err(failure)
    }
    pub(super) fn validate_compute_plan(&self, plan: &Value) -> Result<(), ApiError> {
        // Host resource accounting still owns these terms. Do not sell a catalog
        // variant that the existing compute settlement policy cannot represent.
        if plan["amount"] != "40"
            || plan["currency"] != "usd"
            || plan["interval"] != "month"
            || plan["kind"] != "subscription"
        {
            return Err(ApiError::conflict("billing_compute_catalog_mismatch"));
        }
        if let Some(price) = &self.stripe_cloud_price {
            if plan["stripePriceId"] != *price {
                return Err(ApiError::conflict("billing_compute_catalog_mismatch"));
            }
        }
        // Enrolled accounts pay usage from the shared wallet, never a Stripe meter.
        if plan["stripeMeterPriceIds"]
            .as_object()
            .is_some_and(|m| !m.is_empty())
            || plan["items"]
                .as_array()
                .is_some_and(|items| items.iter().any(|i| i["overage"] == true))
        {
            return Err(ApiError::conflict("billing_compute_catalog_mismatch"));
        }
        let wallet = plan["items"]
            .as_array()
            .and_then(|items| items.iter().find(|i| i["feature"] == CREDIT_WALLET));
        if !wallet
            .is_some_and(|i| i["included"] == "40" && i["reset"] == "month" && i["pool"].is_null())
        {
            return Err(ApiError::conflict("billing_shared_allowance_required"));
        }
        Ok(())
    }
    pub(super) async fn enrolled(&self, pool: &PgPool, owner: &str) -> Result<bool, ApiError> {
        // Always read the durable marker, including accounts removed from the allowlist.
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM zuse_brokeboy_accounts WHERE account_id=$1")
                .bind(owner)
                .fetch_optional(pool)
                .await?;
        if state.is_some() && (!self.accounts.contains(owner) || state.as_deref() != Some("active"))
        {
            return Err(ApiError::conflict("billing_enrollment_incomplete"));
        }
        Ok(state.is_some())
    }
}
impl BillingControl {
    pub fn with_native_billing(mut self, billing: NativeBilling) -> Self {
        self.native = Some(Arc::new(billing));
        self
    }
    pub async fn connect_native_from_env(self, database_url: &str) -> anyhow::Result<Self> {
        match NativeBilling::from_env(database_url).await? {
            Some(native) => Ok(self.with_native_billing(native)),
            None => {
                // A removed configuration must never silently reactivate a legacy writer.
                let exists: bool =
                    sqlx::query_scalar("SELECT to_regclass('zuse_brokeboy_accounts') IS NOT NULL")
                        .fetch_one(&self.pool)
                        .await?;
                if exists {
                    let count: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM zuse_brokeboy_accounts WHERE account_id=ANY($1)",
                    )
                    .bind(self.accounts.iter().cloned().collect::<Vec<_>>())
                    .fetch_one(&self.pool)
                    .await?;
                    anyhow::ensure!(
                        count == 0,
                        "Embedded billing enrollment exists; restore ZUSE_BROKEBOY_ACCOUNTS before starting the billing writer"
                    );
                }
                Ok(self)
            }
        }
    }
    pub async fn close_native_billing(&self) -> Result<(), ApiError> {
        if let Some(native) = &self.native {
            native.close().await?;
        }
        Ok(())
    }
    pub async fn publish_native_catalog(
        &self,
        catalog: Value,
        expected_revision: i64,
    ) -> Result<Value, ApiError> {
        self.native
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?
            .publish_catalog(catalog, expected_revision)
            .await
    }
    pub async fn native_operation(
        &self,
        owner: &str,
        operation: &str,
        retry: bool,
    ) -> Result<Value, ApiError> {
        let native = self
            .native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        let tx = self.owned_transaction(owner).await?;
        let mut input = json!({"customerId":owner,"operationId":operation});
        if retry {
            input["idempotencyKey"] = json!(uuid::Uuid::new_v4().to_string());
        }
        let result = native
            .engine
            .call(
                if retry {
                    "operations.retry"
                } else {
                    "operations.get"
                },
                input,
            )
            .await
            .map_err(failure)?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn native_maintenance(&self) -> Result<Value, ApiError> {
        let Some(native) = &self.native else {
            return Ok(json!({"enabled":false}));
        };
        let mut reports = vec![];
        for owner in &native.accounts {
            let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_brokeboy_accounts WHERE account_id=$1 AND state='active')").bind(owner).fetch_one(&self.pool).await?;
            if !active {
                continue;
            }
            self.recover_model_requests(owner).await?;
            let tx = self.owned_transaction(owner).await?;
            let result = native
                .engine
                .call("maintenance.run", json!({"customerId":owner,"limit":100}))
                .await
                .map_err(failure)?;
            tx.commit().await?;
            reports.push(result);
            self.sync_native_subscriptions(owner).await?;
            self.settle_compute_credits(owner).await?;
            self.sync_expired_model_requests(owner).await?;
            self.sync_credit_capacity(owner).await?;
        }
        Ok(json!({"enabled":true,"reports":reports}))
    }
}
impl crate::store::Store {
    pub async fn close_native_billing(&self) -> Result<(), ApiError> {
        if let Some(control) = &self.billing {
            control.close_native_billing().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
