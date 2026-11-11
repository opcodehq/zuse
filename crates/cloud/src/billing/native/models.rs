//! Trusted hosted-model accounting. BYO credentials never call this interface.
use super::*;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReservation {
    pub request_id: String,
    pub chat_id: String,
    pub model: String,
    pub estimate_micros: i64,
}
fn amount(micros: i64) -> Result<String, ApiError> {
    if !(0..=9_007_199_254_740_991).contains(&micros) {
        return Err(ApiError::bad_request("invalid_model_cost"));
    }
    Ok(format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000))
}
fn key(request: &str, action: &str) -> String {
    format!("model:{:x}:{action}", Sha256::digest(request.as_bytes()))
}
impl BillingControl {
    /// Extend funding before asking the provider to spend beyond the existing hold.
    pub async fn increase_hosted_model(
        &self,
        owner: &str,
        request: &str,
        total_micros: i64,
    ) -> Result<(), ApiError> {
        amount(total_micros)?;
        self.settle_compute_credits(owner).await?;
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_brokeboy_compute_charges WHERE account_id=$1 AND state IN ('pending','reconciliation'))").bind(owner).fetch_one(&self.pool).await?;
        if pending {
            return Err(ApiError::forbidden("shared_credit_exhausted"));
        }
        self.drive_model_request(owner, request).await?;
        let mut tx = self.owned_transaction(owner).await?;
        let row: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_brokeboy_model_requests r WHERE account_id=$1 AND request_id=$2 FOR UPDATE").bind(owner).bind(request).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("model_request_not_found"))?;
        if row["state"] != "held" {
            return Err(ApiError::conflict("model_request_finished"));
        }
        let held = row["held_micros"]
            .as_i64()
            .ok_or_else(|| ApiError::unavailable("invalid_billing_result"))?;
        if total_micros <= held {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE zuse_brokeboy_model_requests SET state='increasing',increase_target_micros=$3 WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).bind(total_micros).execute(&mut *tx).await?;
        tx.commit().await?;
        self.drive_model_request(owner, request).await
    }
    pub async fn reserve_hosted_model(
        &self,
        owner: &str,
        request: &ModelReservation,
        now: i64,
    ) -> Result<String, ApiError> {
        if request.request_id.is_empty()
            || request.request_id.len() > 1024
            || request.chat_id.is_empty()
            || request.chat_id.len() > 1024
            || request.model.is_empty()
            || request.model.len() > 256
            || request.estimate_micros <= 0
        {
            return Err(ApiError::bad_request("invalid_model_reservation"));
        }
        amount(request.estimate_micros)?;
        self.native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        self.settle_compute_credits(owner).await?;
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_brokeboy_compute_charges WHERE account_id=$1 AND state IN ('pending','reconciliation'))").bind(owner).fetch_one(&self.pool).await?;
        if pending {
            return Err(ApiError::forbidden("shared_credit_exhausted"));
        }
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("INSERT INTO zuse_brokeboy_model_requests(account_id,request_id,chat_id,model,estimate_micros,held_micros,state,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$5,'reserving',$6,$7) ON CONFLICT DO NOTHING").bind(owner).bind(&request.request_id).bind(&request.chat_id).bind(&request.model).bind(request.estimate_micros).bind(now).bind(now.checked_add(3_600_000).ok_or_else(||ApiError::bad_request("invalid_model_reservation"))?).execute(&mut *tx).await?;
        let row: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_brokeboy_model_requests r WHERE account_id=$1 AND request_id=$2 FOR UPDATE").bind(owner).bind(&request.request_id).fetch_one(&mut *tx).await?;
        if row["chat_id"] != request.chat_id
            || row["model"] != request.model
            || row["estimate_micros"] != request.estimate_micros
        {
            return Err(ApiError::conflict("billing_idempotency_conflict"));
        }
        if row["state"] != "held" && row["state"] != "reserving" {
            return Err(ApiError::conflict("model_request_finished"));
        }
        tx.commit().await?;
        self.drive_model_request(owner, &request.request_id).await?;
        let row: (String,i64) = sqlx::query_as("SELECT reservation_id,expires_at FROM zuse_brokeboy_model_requests WHERE account_id=$1 AND request_id=$2 AND state='held'").bind(owner).bind(&request.request_id).fetch_optional(&self.pool).await?.ok_or_else(|| ApiError::conflict("model_request_finished"))?;
        if row.1 <= now {
            return Err(ApiError::conflict("model_reservation_expired"));
        }
        Ok(row.0)
    }
    pub async fn settle_hosted_model(
        &self,
        owner: &str,
        request: &str,
        actual_micros: i64,
    ) -> Result<(), ApiError> {
        amount(actual_micros)?;
        self.finish_model_request(owner, request, Some(actual_micros))
            .await
    }
    /// Release only when the provider did no billable work; partial work must settle actuals.
    pub async fn release_hosted_model(&self, owner: &str, request: &str) -> Result<(), ApiError> {
        self.finish_model_request(owner, request, None).await
    }
    async fn finish_model_request(
        &self,
        owner: &str,
        request: &str,
        actual: Option<i64>,
    ) -> Result<(), ApiError> {
        self.native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        self.drive_model_request(owner, request).await?;
        let mut tx = self.owned_transaction(owner).await?;
        let row: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_brokeboy_model_requests r WHERE account_id=$1 AND request_id=$2 FOR UPDATE").bind(owner).bind(request).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("model_request_not_found"))?;
        let desired = if actual.is_some() {
            "settling"
        } else {
            "releasing"
        };
        let finished = if actual.is_some() {
            "settled"
        } else {
            "released"
        };
        if row["state"] == "reconciliation" {
            return Err(ApiError::conflict("model_cost_reconciliation_required"));
        }
        if row["state"] != "held" && !(row["state"] == "estimated" && actual.is_some()) {
            if (row["state"] != desired && row["state"] != finished)
                || row["actual_micros"] != json!(actual)
            {
                return Err(ApiError::conflict("billing_idempotency_conflict"));
            }
        } else {
            sqlx::query("UPDATE zuse_brokeboy_model_requests SET state=$3,actual_micros=$4 WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).bind(desired).bind(actual).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.drive_model_request(owner, request).await
    }
    pub(super) async fn recover_model_requests(&self, owner: &str) -> Result<(), ApiError> {
        let requests: Vec<String> = sqlx::query_scalar("SELECT request_id FROM zuse_brokeboy_model_requests WHERE account_id=$1 AND state IN ('reserving','increasing','settling','releasing') ORDER BY created_at LIMIT 100").bind(owner).fetch_all(&self.pool).await?;
        for request in requests {
            if let Err(error) = self.drive_model_request(owner, &request).await {
                tracing::warn!(code = error.code, "Hosted model billing recovery deferred");
            }
        }
        Ok(())
    }
    pub(super) async fn sync_expired_model_requests(&self, owner: &str) -> Result<(), ApiError> {
        let native = self
            .native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        let expired:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_brokeboy_model_requests WHERE account_id=$1 AND state='held' AND expires_at<=$2)").bind(owner).bind(chrono::Utc::now().timestamp_millis()).fetch_one(&self.pool).await?;
        if !expired {
            return Ok(());
        }
        let reservations = native
            .engine
            .call("reservations.list", json!({"customerId":owner}))
            .await
            .map_err(failure)?;
        let mut tx = self.owned_transaction(owner).await?;
        for reservation in reservations
            .as_array()
            .ok_or_else(|| ApiError::unavailable("invalid_billing_result"))?
        {
            let status = reservation["status"].as_str().unwrap_or_default();
            let state = match status {
                "settled" => "estimated",
                "reconciliation" => "reconciliation",
                _ => continue,
            };
            sqlx::query("UPDATE zuse_brokeboy_model_requests SET state=$3 WHERE account_id=$1 AND reservation_id=$2 AND state='held' AND expires_at<=$4").bind(owner).bind(text(reservation,"id")?).bind(state).bind(chrono::Utc::now().timestamp_millis()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    async fn drive_model_request(&self, owner: &str, request: &str) -> Result<(), ApiError> {
        let native = self
            .native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        // Host row serializes the handoff; engine idempotency recovers a committed debit
        // if the host transaction fails. It never locks a host table in the other pool.
        let mut tx = self.owned_transaction(owner).await?;
        let row: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_brokeboy_model_requests r WHERE account_id=$1 AND request_id=$2 FOR UPDATE").bind(owner).bind(request).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("model_request_not_found"))?;
        let state = text(&row, "state")?;
        if state == "reserving" {
            if row["expires_at"].as_i64().unwrap_or(0) <= chrono::Utc::now().timestamp_millis() {
                sqlx::query("UPDATE zuse_brokeboy_model_requests SET state='failed' WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).execute(&mut *tx).await?;
                tx.commit().await?;
                return Err(ApiError::conflict("model_reservation_expired"));
            }
            let result = match native.engine.call("reservations.create", json!({"customerId":owner,"feature":CREDIT_WALLET,"amount":amount(row["estimate_micros"].as_i64().unwrap_or(-1))?,"expiresAt":row["expires_at"],"onExpiry":"settle_estimate","idempotencyKey":key(request,"reserve")})).await {
                Ok(result)=>result,
                Err(error) if error.code=="insufficient_balance" => {
                    sqlx::query("UPDATE zuse_brokeboy_model_requests SET state='failed' WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).execute(&mut *tx).await?;
                    tx.commit().await?;
                    return Err(failure(error));
                }
                Err(error)=>return Err(failure(error)),
            };
            sqlx::query("UPDATE zuse_brokeboy_model_requests SET reservation_id=$3,state='held' WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).bind(text(&result,"id")?).execute(&mut *tx).await?;
        } else if state == "increasing" {
            let target = row["increase_target_micros"]
                .as_i64()
                .ok_or_else(|| ApiError::bad_request("invalid_model_cost"))?;
            let held = row["held_micros"]
                .as_i64()
                .ok_or_else(|| ApiError::bad_request("invalid_model_cost"))?;
            if let Err(error)=native.engine.call("reservations.increase",json!({"customerId":owner,"reservationId":row["reservation_id"],"amount":amount(target-held)?,"idempotencyKey":key(request,&format!("increase:{target}"))})).await {
                if error.code=="insufficient_balance" {
                    sqlx::query("UPDATE zuse_brokeboy_model_requests SET state='held',increase_target_micros=NULL WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).execute(&mut *tx).await?;
                    tx.commit().await?;
                }
                return Err(failure(error));
            }
            sqlx::query("UPDATE zuse_brokeboy_model_requests SET state='held',held_micros=$3,increase_target_micros=NULL WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).bind(target).execute(&mut *tx).await?;
        } else if matches!(state, "settling" | "releasing") {
            let mut input = json!({"customerId":owner,"reservationId":row["reservation_id"],"idempotencyKey":key(request,if state=="settling" {"settle"} else {"release"})});
            if state == "settling" {
                input["amount"] = json!(amount(row["actual_micros"].as_i64().unwrap_or(-1))?);
            }
            let result = native
                .engine
                .call(
                    if state == "settling" {
                        "reservations.settle"
                    } else {
                        "reservations.release"
                    },
                    input,
                )
                .await
                .map_err(failure)?;
            // Late/uncovered actuals remain visible for reconciliation; never call them settled.
            if result["status"] == "reconciliation" {
                sqlx::query("UPDATE zuse_brokeboy_model_requests SET state='reconciliation' WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).execute(&mut *tx).await?;
                tx.commit().await?;
                return Err(ApiError::conflict("model_cost_reconciliation_required"));
            }
            sqlx::query("UPDATE zuse_brokeboy_model_requests SET state=$3 WHERE account_id=$1 AND request_id=$2").bind(owner).bind(request).bind(if state=="settling" {"settled"} else {"released"}).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.sync_credit_capacity(owner).await?;
        Ok(())
    }
}
impl Store {
    pub async fn hosted_model_usage(
        &self,
        owner: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Value, ApiError> {
        if cursor.is_some_and(|c| c.is_empty() || c.len() > 1024) {
            return Err(ApiError::bad_request("invalid_billing_cursor"));
        }
        let control = self
            .billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?;
        control
            .native_for(owner)
            .await?
            .ok_or_else(|| ApiError::unavailable("embedded_billing_disabled"))?;
        let limit = limit.clamp(1, 100);
        let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('requestId',request_id,'chatId',chat_id,'model',model,'estimateMicros',estimate_micros,'actualMicros',actual_micros,'state',state,'createdAt',created_at,'expiresAt',expires_at) FROM zuse_brokeboy_model_requests WHERE account_id=$1 AND ($2::text IS NULL OR request_id>$2) ORDER BY request_id LIMIT $3").bind(owner).bind(cursor).bind((limit+1) as i64).fetch_all(&control.pool).await?;
        let next = if rows.len() > limit {
            rows[limit - 1]["requestId"].clone()
        } else {
            Value::Null
        };
        Ok(
            json!({"items":rows.into_iter().take(limit).collect::<Vec<_>>(),"nextCursor":next,"currency":"usd"}),
        )
    }
    pub async fn increase_hosted_model(
        &self,
        owner: &str,
        request: &str,
        total_micros: i64,
    ) -> Result<(), ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .increase_hosted_model(owner, request, total_micros)
            .await
    }
    /// Server integration point for Zuse-funded inference, never client-provided usage.
    pub async fn reserve_hosted_model(
        &self,
        owner: &str,
        request: &ModelReservation,
        now: i64,
    ) -> Result<String, ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .reserve_hosted_model(owner, request, now)
            .await
    }
    pub async fn settle_hosted_model(
        &self,
        owner: &str,
        request: &str,
        actual_micros: i64,
    ) -> Result<(), ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .settle_hosted_model(owner, request, actual_micros)
            .await
    }
    pub async fn release_hosted_model(&self, owner: &str, request: &str) -> Result<(), ApiError> {
        self.billing
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_billing_writes_disabled"))?
            .release_hosted_model(owner, request)
            .await
    }
}
