//! Shared durable Stripe delivery. Existing credit and meter receipts remain authoritative.
use super::BillingControl;
use crate::error::ApiError;
use sqlx::Row;
impl BillingControl {
    pub(super) async fn deliver_stripe<F, Fut>(
        &self,
        key: &str,
        payload: &str,
        now: i64,
        reconcile: fn() -> ApiError,
        send: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), ApiError>>,
    {
        let mut tx = self.effects_pool.begin().await?;
        let row=sqlx::query("INSERT INTO api_stripe_meter_deliveries(delivery_key,payload,first_attempt_at,lease_until) VALUES($1,$2,$3,0) ON CONFLICT(delivery_key) DO UPDATE SET delivery_key=EXCLUDED.delivery_key RETURNING payload,sent_at,first_attempt_at,lease_until").bind(key).bind(payload).bind(now).fetch_one(&mut *tx).await?;
        if row.try_get::<String, _>("payload")? != payload {
            return Err(reconcile());
        }
        if row.try_get::<Option<i64>, _>("sent_at")?.is_some() {
            tx.commit().await?;
            return Ok(());
        }
        if now.saturating_sub(row.try_get::<i64, _>("first_attempt_at")?) >= 23 * 3600000 {
            return Err(reconcile());
        }
        if row.try_get::<i64, _>("lease_until")? > now {
            return Err(ApiError::unavailable("billing_delivery_busy"));
        }
        sqlx::query("UPDATE api_stripe_meter_deliveries SET lease_until=$2 WHERE delivery_key=$1")
            .bind(key)
            .bind(now + 60000)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let result = send().await;
        sqlx::query("UPDATE api_stripe_meter_deliveries SET lease_until=0,sent_at=CASE WHEN $2 THEN $3 ELSE sent_at END WHERE delivery_key=$1").bind(key).bind(result.is_ok()).bind(now).execute(&self.effects_pool).await?;
        result
    }
}
