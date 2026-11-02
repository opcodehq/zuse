//! Original retained-image pricing and checkpoint records. No provider wake or deletion.
use super::{
    BillingControl, invalid, number,
    settlement::{UsageIdentity, settle_segment},
};
use crate::error::ApiError;
use serde::Serialize;
use serde_json::{Value, json};
const MONTH_MS: i64 = 2_592_000_000;
const MONTH_MICROS: i64 = 1_700_000;
const VERSION: &str = "boat-snapshot-2026-10-v1";
const DAY: i64 = 86_400_000;
const MAX: i64 = 9_007_199_254_740_991;
#[derive(Clone, Copy)]
pub struct SnapshotSettlementConfig {
    pub cutover_at_ms: i64,
}
impl SnapshotSettlementConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let raw = std::env::var("CLOUD_SNAPSHOT_BILLING_CUTOVER_AT").unwrap_or_default();
        if raw.trim().is_empty() {
            return Ok(None);
        }
        let cutover_at_ms = chrono::DateTime::parse_from_rfc3339(&raw)?.timestamp_millis();
        anyhow::ensure!(
            (1..=MAX).contains(&cutover_at_ms),
            "Snapshot billing cutover must be a positive timestamp"
        );
        Ok(Some(Self { cutover_at_ms }))
    }
}
#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSettlementReport {
    pub accounts: usize,
    pub settled: usize,
    pub skipped: usize,
    pub deferred: usize,
    pub errors: Vec<String>,
}
fn storage_cost(duration: i64, remainder: i64, rate: i64) -> Result<(i64, i64), ApiError> {
    if !(0..=MAX).contains(&duration)
        || !(0..MONTH_MS).contains(&remainder)
        || !(0..=MAX).contains(&rate)
    {
        return Err(invalid());
    }
    let numerator = i128::from(duration) * i128::from(rate) + i128::from(remainder);
    let micros = numerator / i128::from(MONTH_MS);
    if micros > i128::from(MAX) {
        return Err(invalid());
    }
    Ok((micros as i64, (numerator % i128::from(MONTH_MS)) as i64))
}
impl BillingControl {
    pub async fn settle_snapshot_storage_once(
        &self,
        config: SnapshotSettlementConfig,
        now: i64,
    ) -> Result<SnapshotSettlementReport, ApiError> {
        if !(1..=MAX).contains(&config.cutover_at_ms) || !(1..=MAX).contains(&now) {
            return Err(invalid());
        }
        let allowed = self.accounts.iter().cloned().collect::<Vec<_>>();
        let accounts:Vec<String>=sqlx::query_scalar("SELECT DISTINCT account_id FROM api_cloud_snapshots WHERE provider='box' AND account_id=ANY($1) AND (state<>'deleted' OR ((record->>'retainedAtMs')::bigint IS NOT NULL AND COALESCE((record->>'checkpointAtMs')::bigint,(record->>'retainedAtMs')::bigint)<(record->>'stoppedAtMs')::bigint)) ORDER BY account_id").bind(allowed).fetch_all(&self.pool).await?;
        let mut report = SnapshotSettlementReport::default();
        for account in accounts {
            report.accounts += 1;
            match self.settle_snapshot_account(&account, config, now).await {
                Ok((settled, skipped)) => {
                    report.settled += settled;
                    report.skipped += skipped;
                }
                Err(error) => {
                    report.deferred += 1;
                    report.errors.push(format!("{account}:{}", error.code));
                }
            }
        }
        Ok(report)
    }
    async fn settle_snapshot_account(
        &self,
        account: &str,
        config: SnapshotSettlementConfig,
        now: i64,
    ) -> Result<(usize, usize), ApiError> {
        let mut tx = self.owned_transaction(account).await?;
        // Match the reference cross-process lifecycle lock. Holding it through this
        // short DB-only transaction also prevents new shared/exclusive leases.
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("snapshot:{account}"))
                .fetch_one(&mut *tx)
                .await?;
        if !locked {
            return Err(ApiError::conflict("cloud_snapshot_busy"));
        }
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_snapshot_leases WHERE account_id=$1 AND expires_at>floor(extract(epoch FROM clock_timestamp())*1000)::bigint)").bind(account).fetch_one(&mut *tx).await?;
        if busy {
            return Err(ApiError::conflict("cloud_snapshot_busy"));
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(account)
            .execute(&mut *tx)
            .await?;
        let records:Vec<(String,String,String,Value)>=sqlx::query_as("SELECT snapshot_id,build_id,state,record FROM api_cloud_snapshots WHERE provider='box' AND account_id=$1 ORDER BY created_at,snapshot_id FOR UPDATE").bind(account).fetch_all(&mut *tx).await?;
        let (mut settled, mut skipped) = (0, 0);
        for (id, build, state, mut record) in records {
            if record["accountId"] != account
                || record["snapshotId"] != id
                || record["buildId"] != build
                || record["state"] != state
                || record["provider"] != "box"
            {
                return Err(invalid());
            }
            let Some(retained) = record["retainedAtMs"].as_i64() else {
                skipped += 1;
                continue;
            };
            if state == "creating" {
                skipped += 1;
                continue;
            }
            let old_checkpoint = record["checkpointAtMs"].as_i64().unwrap_or(retained);
            let stopped = record["stoppedAtMs"].as_i64();
            if state == "deleted" && stopped.is_none() {
                return Err(invalid());
            }
            let end = now.min(stopped.unwrap_or(now));
            let mut cursor = old_checkpoint.max(config.cutover_at_ms);
            if end <= cursor {
                let checkpoint = old_checkpoint.max(end);
                if checkpoint != old_checkpoint {
                    record["checkpointAtMs"] = json!(checkpoint);
                    save_checkpoint(&mut tx, account, &id, &record).await?;
                }
                skipped += 1;
                continue;
            }
            if stopped.is_none() && end - cursor < 3_600_000 {
                skipped += 1;
                continue;
            }
            // Imported customer-owned builds cannot create platform storage charges.
            let settings:Option<(String,Value)>=sqlx::query_as("SELECT account_id,settings FROM api_cloud_project_builds WHERE build_id=$1 FOR SHARE").bind(&build).fetch_optional(&mut *tx).await?;
            if let Some((owner, settings)) = settings {
                if owner != account {
                    return Err(invalid());
                }
                if settings["providerConnectionId"].is_string() {
                    skipped += 1;
                    continue;
                }
            }
            let price:Option<(i64,i64)>=sqlx::query_as("SELECT monthly_micros,duration_ms FROM api_snapshot_price_schedule WHERE provider='box' AND version=$1 FOR SHARE").bind(VERSION).fetch_optional(&mut *tx).await?;
            if price != Some((MONTH_MICROS, MONTH_MS)) {
                return Err(ApiError::conflict(
                    "snapshot_price_schedule_missing_or_changed",
                ));
            }
            let periods:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<$3 AND period_end>$2 ORDER BY period_start,period_id FOR UPDATE").bind(account).bind(cursor).bind(end).fetch_all(&mut *tx).await?;
            let mut remainder = number(&record, "remainder")?;
            for period in periods {
                let start = cursor.max(number(&period, "period_start")?);
                let until = end.min(number(&period, "period_end")?);
                if until <= start {
                    continue;
                }
                if period["currency"] != "USD" {
                    return Err(invalid());
                }
                if start > cursor {
                    remainder = 0;
                }
                let (cost, next_remainder) = storage_cost(until - start, remainder, MONTH_MICROS)?;
                let event = format!("snapshot:{id}:{VERSION}:{start}:{until}");
                sqlx::query("INSERT INTO api_provider_usage_events(provider,event_id,type,provider_resource_id,payload,received_at,occurred_at,expires_at) VALUES('box',$1,'snapshot-storage',$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
                    .bind(&event).bind(&id).bind(json!({"priceVersion":VERSION,"start":start,"until":until})).bind(now).bind(until).bind(now+90*DAY).execute(&mut *tx).await?;
                let finalized:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_provider_event_finalizations WHERE provider='box' AND (event_id=$1 OR provider_execution_id=$1))").bind(&event).fetch_one(&mut *tx).await?;
                if !finalized {
                    let entry = format!("box:{event}");
                    let resource = json!({"accountId":account,"kind":"snapshot","id":id});
                    settle_segment(
                        &mut tx,
                        &resource,
                        UsageIdentity {
                            provider: "box",
                            entry: &entry,
                            event: &event,
                            execution: &event,
                            machine: None,
                        },
                        &period,
                        start,
                        until,
                        cost,
                        0,
                        0,
                        now,
                    )
                    .await?;
                    sqlx::query("INSERT INTO api_provider_event_finalizations(provider,event_id,provider_execution_id,finalized_at,expires_at) VALUES('box',$1,$1,$2,$3)").bind(&event).bind(now).bind(now+7*365*DAY).execute(&mut *tx).await?;
                    settled += 1;
                }
                remainder = next_remainder;
                cursor = until;
            }
            record["checkpointAtMs"] = json!(cursor);
            record["remainder"] = json!(remainder);
            save_checkpoint(&mut tx, account, &id, &record).await?;
        }
        tx.commit().await?;
        Ok((settled, skipped))
    }
}
async fn save_checkpoint(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &str,
    id: &str,
    record: &Value,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE api_cloud_snapshots SET record=$3 WHERE provider='box' AND account_id=$1 AND snapshot_id=$2").bind(account).bind(id).bind(record).execute(&mut **tx).await?;
    Ok(())
}
#[cfg(test)]
mod tests;
