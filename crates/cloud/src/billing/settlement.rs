//! Completed provider reports settle original executions once, under the billing writer fence.
use super::{BillingControl, invalid, number};
use crate::{error::ApiError, providers::boxd::Boxd};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
const DAY: i64 = 86_400_000;
const MAX: i64 = 9_007_199_254_740_991;

#[derive(Clone, Copy)]
pub struct BoxdSettlementConfig {
    pub cutover_at_ms: i64,
}
impl BoxdSettlementConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        if std::env::var("BOXD_BILLING_ENABLED").as_deref() != Ok("true") {
            return Ok(None);
        }
        let raw = std::env::var("BOXD_BILLING_CUTOVER_AT")?;
        let cutover_at_ms = chrono::DateTime::parse_from_rfc3339(&raw)?.timestamp_millis();
        anyhow::ensure!(
            cutover_at_ms > 0 && cutover_at_ms % 1000 == 0,
            "Boxd billing cutover must be a positive whole second"
        );
        if std::env::var("BOXD_ADAPTER_ENABLED").as_deref() != Ok("true") {
            return Ok(None);
        }
        Ok(Some(Self { cutover_at_ms }))
    }
}
#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementReport {
    pub windows: usize,
    pub settled: usize,
    pub skipped: usize,
    pub deferred: usize,
    pub errors: Vec<String>,
}
fn pending() -> ApiError {
    ApiError::conflict("provider_cost_settlement_pending")
}
fn windows(cutover: i64, now: i64) -> Result<Vec<(i64, i64)>, ApiError> {
    if cutover <= 0 || cutover % 1000 != 0 || now > MAX {
        return Err(invalid());
    }
    let end = (now - 30 * 60_000).div_euclid(DAY) * DAY;
    let mut all = Vec::new();
    let mut start = cutover;
    while start < end {
        let next = ((start / DAY + 1) * DAY).min(end);
        all.push((start, next));
        start = next;
    }
    if all.is_empty() {
        return Ok(all);
    }
    let batch = (now.max(0) / 60_000) as usize % all.len().div_ceil(7);
    let mut selected = all
        .iter()
        .skip(batch * 7)
        .take(7)
        .copied()
        .collect::<Vec<_>>();
    let latest = *all.last().unwrap();
    if !selected.contains(&latest) {
        selected.push(latest);
    }
    Ok(selected)
}
impl BillingControl {
    pub async fn settle_boxd_once(
        &self,
        provider: &Boxd,
        config: BoxdSettlementConfig,
        now: i64,
    ) -> Result<SettlementReport, ApiError> {
        let mut report = SettlementReport::default();
        for (start, end) in windows(config.cutover_at_ms, now)? {
            report.windows += 1;
            let machines = match provider.organization_usage(start, end).await {
                Ok(machines) => machines,
                Err(_) => {
                    report.deferred += 1;
                    report
                        .errors
                        .push(format!("window:{start}:{end}:provider_usage_unavailable"));
                    continue;
                }
            };
            for machine in machines {
                if !machine.complete {
                    report.skipped += 1;
                    continue;
                }
                let event = format!("usage:{}:{start}:{end}", machine.machine_id);
                if finalized(&self.pool, &event).await? {
                    report.skipped += 1;
                    continue;
                }
                let Some(resource) = resource(&self.pool, &machine.machine_id).await? else {
                    report.skipped += 1;
                    continue;
                };
                let owner = resource["accountId"].as_str().ok_or_else(invalid)?;
                if !self.accounts.contains(owner)
                    || resource["config"]["providerConnectionId"].is_string()
                {
                    report.skipped += 1;
                    continue;
                }
                match self
                    .settle_boxd_execution(
                        provider,
                        &resource,
                        &machine.machine_id,
                        &event,
                        start,
                        end,
                        config.cutover_at_ms,
                        now,
                    )
                    .await
                {
                    Ok(true) => report.settled += 1,
                    Ok(false) => report.skipped += 1,
                    Err(error) => {
                        report.deferred += 1;
                        report.errors.push(format!("{event}:{}", error.code));
                    }
                }
            }
        }
        Ok(report)
    }
    async fn settle_boxd_execution(
        &self,
        provider: &Boxd,
        resource: &Value,
        machine: &str,
        event: &str,
        window_start: i64,
        end: i64,
        cutover: i64,
        now: i64,
    ) -> Result<bool, ApiError> {
        let owner = resource["accountId"].as_str().ok_or_else(invalid)?;
        let created = number(resource, "createdAt")?;
        let start = window_start.max(created).max(cutover);
        if start >= end {
            return Ok(false);
        }
        // Retain the original window event even if period coverage/provider cost is not ready.
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("INSERT INTO api_provider_usage_events(provider,event_id,type,provider_resource_id,payload,received_at,expires_at) VALUES('boxd',$1,'boxd.usage.window',$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(event).bind(machine).bind(json!({"id":event,"machineId":machine,"startedAtMs":window_start,"endedAtMs":end})).bind(now).bind(now+90*DAY).execute(&mut *tx).await?;
        tx.commit().await?;
        let periods = periods(&self.pool, owner, start, end).await?;
        let segments = segments(&periods, start, end)?;
        let mut costs = Vec::new();
        for (a, b) in &segments {
            let request_start = if *a == created {
                created / 900_000 * 900_000
            } else {
                *a
            };
            costs.push(
                provider
                    .machine_usage(machine, request_start, *b)
                    .await
                    .map_err(|_| pending())?,
            );
        }
        let mut tx = self.owned_transaction(owner).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        // Resource identity/config and period terms cannot change between report reads and settlement.
        let table = if resource["kind"] == "workspace" {
            "api_cloud_workspaces"
        } else {
            "api_cloud_project_builds"
        };
        let id_column = if resource["kind"] == "workspace" {
            "workspace_id"
        } else {
            "build_id"
        };
        let config_column = if resource["kind"] == "workspace" {
            "request_config"
        } else {
            "settings"
        };
        let current:Option<(String,i64,Value)>=sqlx::query_as(&format!("SELECT account_id,created_at,{config_column} FROM {table} WHERE {id_column}=$1 FOR SHARE"))
            .bind(resource["id"].as_str().ok_or_else(invalid)?).fetch_optional(&mut *tx).await?;
        if current != Some((owner.to_owned(), created, resource["config"].clone())) {
            return Err(pending());
        }
        let locked:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<$3 AND period_end>$2 ORDER BY period_start,period_id FOR UPDATE")
            .bind(owner).bind(start).bind(end).fetch_all(&mut *tx).await?;
        if locked != periods {
            return Err(pending());
        }
        let _:String=sqlx::query_scalar("SELECT event_id FROM api_provider_usage_events WHERE provider='boxd' AND event_id=$1 FOR UPDATE").bind(event).fetch_one(&mut *tx).await?;
        if finalized(&mut *tx, event).await? {
            tx.commit().await?;
            return Ok(false);
        }
        for ((period, (a, b)), cost) in periods.iter().zip(segments).zip(costs) {
            let period_id = period["period_id"].as_str().ok_or_else(invalid)?;
            let entry = format!("boxd:{event}:{period_id}");
            let segment_event = format!("{event}:{period_id}");
            settle_segment(
                &mut tx,
                resource,
                UsageIdentity {
                    provider: "boxd",
                    entry: &entry,
                    event: &segment_event,
                    execution: event,
                    machine: Some(machine),
                },
                period,
                a,
                b,
                cost.cost_micros,
                cost.vcpu,
                cost.memory_bytes,
                now,
            )
            .await?;
        }
        sqlx::query("INSERT INTO api_provider_event_finalizations(provider,event_id,provider_execution_id,finalized_at,expires_at) VALUES('boxd',$1,$1,$2,$3)")
            .bind(event).bind(now).bind(now+7*365*DAY).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}
async fn finalized<'e, E: sqlx::Executor<'e, Database = Postgres>>(
    db: E,
    event: &str,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_provider_event_finalizations WHERE provider='boxd' AND (event_id=$1 OR provider_execution_id=$1))").bind(event).fetch_one(db).await?)
}
async fn periods(
    pool: &sqlx::PgPool,
    owner: &str,
    start: i64,
    end: i64,
) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<$3 AND period_end>$2 ORDER BY period_start,period_id").bind(owner).bind(start).bind(end).fetch_all(pool).await?)
}
fn segments(periods: &[Value], start: i64, end: i64) -> Result<Vec<(i64, i64)>, ApiError> {
    let mut cursor = start;
    let mut result = Vec::new();
    for p in periods {
        let a = number(p, "period_start")?.max(start);
        let b = number(p, "period_end")?.min(end);
        if a != cursor || b <= a || p["currency"] != "USD" {
            return Err(pending());
        }
        result.push((a, b));
        cursor = b;
    }
    if cursor != end {
        return Err(pending());
    }
    Ok(result)
}
async fn resource(pool: &sqlx::PgPool, machine: &str) -> Result<Option<Value>, ApiError> {
    // Old retired machine IDs remain attributed through their durable observation.
    let direct:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('accountId',account_id,'kind','workspace','id',workspace_id,'createdAt',created_at,'config',request_config) FROM api_cloud_workspaces WHERE provider='boxd' AND provider_sandbox_id=$1 UNION ALL SELECT jsonb_build_object('accountId',account_id,'kind','build','id',build_id,'createdAt',created_at,'config',settings) FROM api_cloud_project_builds WHERE provider='boxd' AND provider_sandbox_id=$1 LIMIT 1").bind(machine).fetch_optional(pool).await?;
    if direct.is_some() {
        return Ok(direct);
    }
    let observation:Option<Value>=sqlx::query_scalar("SELECT observation FROM api_cloud_runtime_observations WHERE provider='boxd' AND provider_sandbox_id=$1").bind(machine).fetch_optional(pool).await?;
    let Some(o) = observation else {
        return Ok(None);
    };
    let (table, id, config) = match o["resourceKind"].as_str() {
        Some("workspace") => ("api_cloud_workspaces", "workspace_id", "request_config"),
        Some("build") => ("api_cloud_project_builds", "build_id", "settings"),
        _ => return Ok(None),
    };
    let result:Option<Value>=sqlx::query_scalar(&format!("SELECT jsonb_build_object('accountId',account_id,'kind',$2::text,'id',{id},'createdAt',created_at,'config',{config}) FROM {table} WHERE {id}=$1 AND provider='boxd'"))
        .bind(o["resourceId"].as_str()).bind(o["resourceKind"].as_str()).fetch_optional(pool).await?;
    Ok(result.filter(|r| r["accountId"] == o["accountId"]))
}
fn deltas(
    p: &Value,
    before: i64,
    charged: i64,
    cost: i64,
) -> Result<Vec<(&'static str, i64)>, ApiError> {
    let included = number(p, "included_provider_cost_micros")? as i128;
    let markup = number(p, "markup_basis_points")? as i128;
    let cap = number(p, "overage_cap_micros")? as i128;
    if [
        included,
        markup,
        cap,
        before as i128,
        charged as i128,
        cost as i128,
    ]
    .iter()
    .any(|v| *v < 0 || *v > MAX as i128)
    {
        return Err(invalid());
    }
    let b = before as i128;
    let a = b + cost as i128;
    let bo = (b - included).max(0);
    let ao = (a - included).max(0);
    let br = bo * (10000 + markup) / 10000;
    let ar = ao * (10000 + markup) / 10000;
    let bc = charged as i128;
    let ac = if p["status"] == "manual" {
        0
    } else {
        ar.min(cap)
    };
    [
        ("included-allowance", a.min(included) - b.min(included)),
        ("overage-provider-cost", ao - bo),
        ("overage-charge", ac - bc),
        ("markup", (ac - ao).max(0) - (bc - bo).max(0)),
        (
            if p["status"] == "manual" {
                "manual-entitlement-credit"
            } else {
                "overshoot-absorbed"
            },
            (ar - ac) - (br - bc),
        ),
    ]
    .into_iter()
    .map(|(k, v)| {
        if v.abs() > MAX as i128 {
            Err(invalid())
        } else {
            Ok((k, v as i64))
        }
    })
    .collect()
}
pub(super) struct UsageIdentity<'a> {
    pub provider: &'a str,
    pub entry: &'a str,
    pub event: &'a str,
    pub execution: &'a str,
    pub machine: Option<&'a str>,
}
pub(super) async fn settle_segment(
    tx: &mut Transaction<'_, Postgres>,
    resource: &Value,
    identity: UsageIdentity<'_>,
    p: &Value,
    start: i64,
    end: i64,
    cost: i64,
    vcpu: u32,
    memory: u64,
    now: i64,
) -> Result<(), ApiError> {
    let owner = resource["accountId"].as_str().ok_or_else(invalid)?;
    let kind = resource["kind"].as_str().ok_or_else(invalid)?;
    let id = resource["id"].as_str().ok_or_else(invalid)?;
    let period = p["period_id"].as_str().ok_or_else(invalid)?;
    let provider = p["billing_provider"].as_str().unwrap_or("polar");
    if !matches!(provider, "stripe" | "polar") {
        return Err(invalid());
    }
    let entry = identity.entry;
    let (before,charged):(i64,i64)=sqlx::query_as("SELECT COALESCE(SUM(amount_micros) FILTER(WHERE kind='provider-cost'),0)::bigint,COALESCE(SUM(amount_micros) FILTER(WHERE kind='overage-charge'),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1").bind(period).fetch_one(&mut **tx).await?;
    let deltas = deltas(p, before, charged, cost)?;
    let inserted=sqlx::query("INSERT INTO api_cloud_billing_usage(entry_id,period_id,account_id,resource_kind,resource_id,provider,provider_event_id,provider_execution_id,started_at,ended_at,vcpu_count,memory_mib,provider_cost_micros,status,created_at) VALUES($1,$2,$3,$4,$5,$14,$6,$7,$8,$9,$10,$11,$12,'confirmed',$13) ON CONFLICT DO NOTHING")
        .bind(&entry).bind(period).bind(owner).bind(kind).bind(id).bind(identity.event).bind(identity.execution).bind(start).bind(end).bind(i64::from(vcpu)).bind(i64::try_from(memory/1_048_576).map_err(|_|invalid())?).bind(cost).bind(now).bind(identity.provider).execute(&mut **tx).await?.rows_affected();
    if inserted == 0 {
        return Ok(());
    }
    if let Some(machine) = identity.machine {
        let receipt = format!("{}:cost:{start}:{end}:{cost}", identity.execution);
        sqlx::query("INSERT INTO api_provider_usage_events(provider,event_id,type,provider_resource_id,payload,received_at,expires_at) VALUES('boxd',$1,'boxd.usage.cost',$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(receipt).bind(machine).bind(json!({"machineId":machine,"startedAtMs":start,"endedAtMs":end,"costMicro":cost,"currency":"usd","complete":true})).bind(now).bind(now+90*DAY).execute(&mut **tx).await?;
    }
    sqlx::query("DELETE FROM api_cloud_billing_reservations WHERE period_id=$1 AND resource_kind=$2 AND resource_id=$3").bind(period).bind(kind).bind(id).execute(&mut **tx).await?;
    for (k, amount) in std::iter::once(("provider-cost", cost)).chain(deltas) {
        if amount == 0 && k != "provider-cost" {
            continue;
        }
        let ledger = if k == "provider-cost" {
            format!("ledger:{entry}")
        } else {
            format!("ledger:{k}:{entry}")
        };
        let metadata = if k == "provider-cost" {
            json!({"provider":identity.provider,"resourceKind":kind,"resourceId":id})
        } else {
            json!({})
        };
        sqlx::query("INSERT INTO api_cloud_billing_ledger(entry_id,period_id,account_id,kind,amount_micros,source_id,metadata,occurred_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(ledger).bind(period).bind(owner).bind(k).bind(amount).bind(&entry).bind(metadata).bind(end).bind(now).execute(&mut **tx).await?;
    }
    let charged:i64=sqlx::query_scalar("SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='overage-charge'").bind(period).fetch_one(&mut **tx).await?;
    let (exported,count):(i64,i64)=sqlx::query_as("SELECT COALESCE(SUM(amount_cents),0)::bigint,COUNT(*) FROM api_cloud_billing_outbox WHERE period_id=$1").bind(period).fetch_one(&mut **tx).await?;
    let delta = (charged.max(0) + 5000) / 10000 - exported;
    if delta != 0 {
        let key = format!("{period}:{}", count + 1);
        let period_start = number(p, "period_start")?;
        let occurred = (if provider == "stripe" {
            (period_start + 59999) / 60000 * 60000
        } else {
            period_start
        })
        .max(end.min(number(p, "period_end")? - 1000));
        sqlx::query("INSERT INTO api_cloud_billing_outbox(outbox_id,period_id,account_id,provider,amount_cents,idempotency_key,attempt_count,next_attempt_at,created_at,occurred_at) VALUES($1,$2,$3,$4,$5,$6,0,$7,$7,$8)")
            .bind(format!("outbox:{key}")).bind(period).bind(owner).bind(provider).bind(delta).bind(key).bind(now).bind(occurred).execute(&mut **tx).await?;
    }
    let iso = |ms| {
        chrono::DateTime::from_timestamp_millis(ms)
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .ok_or_else(invalid)
    };
    let telemetry_id = format!("provider-cost:{entry}");
    let mut metadata = json!({"provider":identity.provider,"resource_kind":kind,"resource_id":id,"billing_period_id":period,"started_at":iso(start)?,"ended_at":iso(end)?,"currency":"USD","measurement":if identity.provider=="boxd" {"completed-provider-estimate"} else {"confirmed"},"billable":"false"});
    if identity.provider == "boxd" {
        metadata["cost_source"] = json!("provider-current-rates");
        metadata["provider_machine_id"] = json!(identity.machine.unwrap_or(""));
    }
    if kind == "snapshot" {
        metadata["cost_source"] = json!("approved-storage-schedule");
        metadata["provider_snapshot_id"] = json!(id);
    }
    let payload = json!({"eventId":telemetry_id,"accountId":owner,"eventName":"zuse_cloud_provider_cost_micros","units":cost,"occurredAtMs":end,"billingProvider":provider,"metadata":metadata});
    sqlx::query("INSERT INTO api_cloud_usage_outbox(event_id,payload,created_at,next_attempt_at) VALUES($1,$2,$3,$3) ON CONFLICT DO NOTHING").bind(telemetry_id).bind(payload).bind(now).execute(&mut **tx).await?;
    Ok(())
}
#[cfg(test)]
mod tests;
