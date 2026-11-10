use super::*;
use crate::billing::CheckoutSummary;
use brokeboy::core::decimal::Decimal;
use sha2::{Digest, Sha256};
use zuse_protocol::cloud_billing::PrepaidBalance;

impl BillingControl {
    async fn native_payment(
        &self,
        owner: &str,
        key: &str,
        method: &str,
        input: Value,
    ) -> Result<Value, ApiError> {
        let native = self
            .native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("INSERT INTO zuse_brokeboy_checkout_requests(account_id,idempotency_key,method,payload,created_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING").bind(owner).bind(key).bind(method).bind(&input).bind(chrono::Utc::now().timestamp_millis()).execute(&mut *tx).await?;
        let (saved_method,saved): (String,Value) = sqlx::query_as("SELECT method,payload FROM zuse_brokeboy_checkout_requests WHERE account_id=$1 AND idempotency_key=$2").bind(owner).bind(key).fetch_one(&mut *tx).await?;
        if saved_method != method
            || ["planId", "feature", "amountCents", "provider"]
                .iter()
                .any(|k| saved[*k] != input[*k])
        {
            return Err(ApiError::conflict("billing_idempotency_conflict"));
        }
        tx.commit().await?;
        let tx = self.owned_transaction(owner).await?;
        let result = native.engine.call(method, saved).await.map_err(failure)?;
        tx.commit().await?;
        Ok(result)
    }
    /// Explicit enrollment is resumable. Existing Stripe identities require adoption.
    /// Invoice credit stays at Stripe; it is never copied into the model wallet.
    pub async fn enroll_native(
        &self,
        owner: &str,
        adopt_existing: bool,
        now: i64,
    ) -> Result<(), ApiError> {
        let native = self
            .native
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        if !native.accounts.contains(owner) {
            return Err(ApiError::forbidden("cloud_billing_writes_disabled"));
        }
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_brokeboy_accounts WHERE account_id=$1 AND state='active')").bind(owner).fetch_one(&mut *tx).await?;
        if active {
            tx.commit().await?;
            return self.sync_native_subscriptions(owner).await;
        }
        let customer: Option<String> =
            sqlx::query_scalar("SELECT customer_id FROM api_stripe_customers WHERE account_id=$1")
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
        let existing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND provider_subscription_id IS NOT NULL)").bind(owner).fetch_one(&mut *tx).await?;
        if (customer.is_some() || existing) && !adopt_existing {
            return Err(ApiError::conflict("billing_adoption_required"));
        }
        if let Some(customer) = &customer {
            let verifier = self
                .payments
                .as_ref()
                .and_then(|p| p.providers.get("stripe"))
                .ok_or_else(|| ApiError::unavailable("billing_adoption_verifier_required"))?;
            verifier.verify_customer(owner, customer).await?;
        }
        let processors:Vec<String>=sqlx::query_scalar("SELECT DISTINCT provider FROM api_entitlements WHERE account_id=$1 AND provider_subscription_id IS NOT NULL AND status<>'ended'").bind(owner).fetch_all(&mut *tx).await?;
        if processors.len() > 1 {
            return Err(ApiError::conflict("billing_provider_ambiguous"));
        }
        let processor = processors
            .first()
            .cloned()
            .unwrap_or_else(|| native.default_provider.clone());
        sqlx::query("INSERT INTO zuse_brokeboy_accounts(account_id,provider,state,stripe_customer_id,created_at) VALUES($1,$2,'initializing',$3,$4) ON CONFLICT(account_id) DO NOTHING").bind(owner).bind(&processor).bind(&customer).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        // From this commit onward checkout cannot fall back to the old writer.
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("SELECT account_id FROM zuse_brokeboy_accounts WHERE account_id=$1 FOR UPDATE")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        if let Some(customer) = customer {
            native.engine.call("migration.adoptStripeCustomer", json!({"customerId":owner,"stripeCustomerId":customer,"dryRun":false,"idempotencyKey":"zuse-adopt-v1"})).await.map_err(failure)?;
        } else {
            native
                .engine
                .call(
                    "customers.create",
                    json!({"id":owner,"idempotencyKey":"zuse-create-v1"}),
                )
                .await
                .map_err(failure)?;
        }
        let subscriptions: Vec<(String, String, String)> = sqlx::query_as("SELECT provider,provider_subscription_id,offer_id FROM api_entitlements WHERE account_id=$1 AND provider_subscription_id IS NOT NULL AND status<>'ended'").bind(owner).fetch_all(&mut *tx).await?;
        for (provider, id, plan) in subscriptions {
            let mut input = json!({"customerId":owner,"planId":plan,"provider":provider,"idempotencyKey":format!("adopt-sub:{id}")});
            input[if provider == "stripe" {
                "stripeSubscriptionId"
            } else {
                "providerSubscriptionId"
            }] = json!(id);
            let operation = native
                .engine
                .call("migration.linkSubscription", input)
                .await
                .map_err(failure)?;
            completed(&operation)?;
        }
        sqlx::query("UPDATE zuse_brokeboy_accounts SET state='active' WHERE account_id=$1")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.sync_native_subscriptions(owner).await
    }
    pub(super) async fn native_for(&self, owner: &str) -> Result<Option<&NativeBilling>, ApiError> {
        match &self.native {
            Some(native) if native.enrolled(&self.pool, owner).await? => Ok(Some(native)),
            _ => Ok(None),
        }
    }
    pub(in crate::billing) async fn native_checkout(
        &self,
        owner: &str,
        offer: &str,
        now: i64,
        request_key: Option<&str>,
    ) -> Result<Option<String>, ApiError> {
        let Some(native) = self.native_for(owner).await? else {
            return Ok(None);
        };
        if offer != CLOUD_PLAN {
            return Err(ApiError::bad_request("invalid_machine_offer"));
        }
        let plan = native
            .engine
            .call("catalog.get", json!({"id":offer}))
            .await
            .map_err(failure)?;
        native.validate_compute_plan(&plan)?;
        let signer = native
            .signer
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("billing_approval_pending"))?;
        let ticket = signer.checkout(owner, offer, now)?;
        let mut success =
            reqwest::Url::parse(&format!("{}/v1/billing/checkout/complete", signer.issuer()))
                .map_err(|_| ApiError::unavailable("invalid_billing_url"))?;
        success
            .query_pairs_mut()
            .append_pair("offer", offer)
            .append_pair("t", &ticket);
        // The native operation ID is available before provider I/O and survives response loss.
        let key = checkout_key(request_key)?;
        let receipt = format!("payment:{key}");
        // Use a signed opaque receipt key; do not trust a caller-supplied processor session ID.
        let success = format!(
            "{success}&checkout_id={}",
            percent_encoding::utf8_percent_encode(&receipt, percent_encoding::NON_ALPHANUMERIC)
        );
        let provider = account_provider(&self.pool, owner).await?;
        if provider == "stripe"
            && (native.stripe_cloud_price.is_none()
                || (native.compute_metered && native.stripe_overage_price.is_none()))
        {
            return Err(ApiError::unavailable("billing_compute_catalog_mismatch"));
        }
        let operation = self.native_payment(owner,&key,"subscriptions.checkout", json!({"customerId":owner,"planId":offer,"provider":provider,"successUrl":success,"automaticTax":native.automatic_tax,"idempotencyKey":key})).await?;
        self.sync_native_customer(owner).await?;
        Ok(Some(operation_url(&operation)?))
    }
    pub(in crate::billing) async fn native_portal(
        &self,
        owner: &str,
    ) -> Result<Option<String>, ApiError> {
        let Some(native) = self.native_for(owner).await? else {
            return Ok(None);
        };
        let provider = account_provider(&self.pool, owner).await?;
        let tx = self.owned_transaction(owner).await?;
        let operation = native.engine.call("subscriptions.portal", json!({"customerId":owner,"provider":provider,"returnUrl":native.return_origin,"idempotencyKey":uuid::Uuid::new_v4().to_string()})).await.map_err(failure)?;
        tx.commit().await?;
        Ok(Some(operation_url(&operation)?))
    }
    pub(in crate::billing) async fn native_balance(
        &self,
        owner: &str,
    ) -> Result<Option<PrepaidBalance>, ApiError> {
        let Some(native) = self.native_for(owner).await? else {
            return Ok(None);
        };
        let value = native
            .engine
            .call(
                "credits.balance",
                json!({"customerId":owner,"feature":MODEL_WALLET}),
            )
            .await
            .map_err(failure)?;
        let available: Decimal = text(&value, "available")?.parse().map_err(failure)?;
        let micros = available.scaled();
        let mut invoice_credit_cents = None;
        let mut debit_cents = 0;
        if let Some(adapter) = self
            .payments
            .as_ref()
            .and_then(|p| p.providers.get("stripe"))
        {
            let customer: Option<String> = sqlx::query_scalar(
                "SELECT stripe_customer_id FROM zuse_brokeboy_accounts WHERE account_id=$1",
            )
            .bind(owner)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
            if let Some(customer) = customer {
                let legacy = adapter.prepaid_balance(owner, Some(&customer)).await?;
                invoice_credit_cents = Some(legacy.credit_cents);
                debit_cents = legacy.debit_cents;
            }
        }
        Ok(Some(PrepaidBalance {
            available: account_provider(&self.pool, owner).await? == "stripe"
                && native.signer.is_some(),
            credit_cents: i64::try_from(micros / 10000)
                .map_err(|_| ApiError::unavailable("invalid_billing_result"))?,
            debit_cents,
            currency: "usd".into(),
            funding: "hosted-models".into(),
            invoice_credit_cents,
        }))
    }
    pub(in crate::billing) async fn native_prepaid_checkout(
        &self,
        owner: &str,
        cents: i64,
        request_key: Option<&str>,
    ) -> Result<Option<String>, ApiError> {
        let Some(native) = self.native_for(owner).await? else {
            return Ok(None);
        };
        if account_provider(&self.pool, owner).await? != "stripe" {
            return Err(ApiError::unavailable(
                "billing_prepaid_provider_unsupported",
            ));
        }
        if native.signer.is_none() {
            return Err(ApiError::unavailable("billing_approval_pending"));
        }
        let key = checkout_key(request_key)?;
        let operation = self.native_payment(owner,&key,"prepaid.checkout", json!({"customerId":owner,"provider":"stripe","feature":MODEL_WALLET,"amountCents":cents,"successUrl":format!("{}/v1/billing/prepaid/complete?funding=hosted-models",native.return_origin),"idempotencyKey":key})).await?;
        self.sync_native_customer(owner).await?;
        Ok(Some(operation_url(&operation)?))
    }
    pub(in crate::billing) async fn native_receipt(
        &self,
        owner: &str,
        id: &str,
    ) -> Option<CheckoutSummary> {
        let native = self.native_for(owner).await.ok()??;
        let work = native
            .engine
            .call(
                "operations.get",
                json!({"customerId":owner,"operationId":id}),
            )
            .await
            .ok()?;
        if work["kind"] != "checkout" {
            return None;
        }
        completed(&work).ok()?;
        // Operation completion means checkout creation, never proof of payment.
        let subscriptions = native
            .engine
            .call("subscriptions.list", json!({"customerId":owner}))
            .await
            .ok()?;
        let paid = subscriptions.as_array()?.iter().any(|s| {
            s["plan"]["id"] == CLOUD_PLAN
                && s["paidThrough"].as_i64().unwrap_or(0) > chrono::Utc::now().timestamp_millis()
                && matches!(s["status"].as_str(), Some("active" | "trialing"))
        });
        Some(CheckoutSummary {
            checkout_id: id.into(),
            // Native subscription state proves access, not a tax-inclusive receipt total.
            amount_cents: 0,
            currency: "usd".into(),
            created_at_ms: work["createdAt"].as_i64()?,
            status: if paid { "active" } else { "pending" }.into(),
        })
    }
}
async fn account_provider(pool: &PgPool, owner: &str) -> Result<String, ApiError> {
    sqlx::query_scalar(
        "SELECT provider FROM zuse_brokeboy_accounts WHERE account_id=$1 AND state='active'",
    )
    .bind(owner)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::conflict("billing_enrollment_required"))
}
pub(super) fn completed(work: &Value) -> Result<(), ApiError> {
    if work["status"] != "done" {
        return Err(ApiError::unavailable("billing_operation_incomplete"));
    }
    Ok(())
}
fn operation_url(work: &Value) -> Result<String, ApiError> {
    completed(work)?;
    let url = text(&work["result"], "url")?;
    secure_url(url)?;
    Ok(url.into())
}

fn checkout_key(key: Option<&str>) -> Result<String, ApiError> {
    match key {
        Some(k) if k.is_empty() || k.len() > 1024 => {
            Err(ApiError::bad_request("invalid_billing_idempotency_key"))
        }
        Some(k) => Ok(format!("checkout:{:x}", Sha256::digest(k.as_bytes()))),
        None => Ok(uuid::Uuid::new_v4().to_string()),
    }
}
