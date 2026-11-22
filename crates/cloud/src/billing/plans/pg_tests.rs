//! Machine-minute plans over the real schema: trial, abuse limits, metering,
//! pay as you go up to the cap, holds and plan changes.
use super::*;
use crate::billing::{
    BillingControl, Subscription,
    settlement::{UsageIdentity, settle_segment},
};
use crate::store::Store;
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use zuse_protocol::cloud_billing::{BillingCapRequest, plans::names_infrastructure};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
const GIB: u64 = 1 << 30;
const ACCOUNTS: [&str; 6] = [
    "user-trial",
    "user-copycat",
    "user-early",
    "user-cancel",
    "user-cloud",
    "user-max",
];

struct Db {
    admin: PgPool,
    pool: PgPool,
    schema: String,
}
async fn database() -> Db {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("isolated localhost PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("plans_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut scoped = reqwest::Url::parse(&url).unwrap();
    scoped
        .query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(scoped.as_str())
        .await
        .unwrap();
    let wakeups = include_str!("../../../migrations/0043_work_wakeups.sql");
    let notify = &wakeups[..wakeups.find("END $body$;").unwrap() + "END $body$;".len()];
    for sql in [
        include_str!("../../../migrations/0000_application.sql"),
        include_str!("../../../migrations/0001_control_plane.sql"),
        notify,
        include_str!("../../../migrations/0054_machine_minute_plans.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    Db {
        admin,
        pool,
        schema,
    }
}
fn sub(owner: &str, id: &str, offer: &str, trialing: bool, start: i64, end: i64) -> Subscription {
    Subscription {
        owner: owner.into(),
        id: id.into(),
        offer: offer.into(),
        status: "active".into(),
        period_start: start,
        paid_through: end,
        customer: None,
        trialing,
        card_fingerprint: Some(format!("fp-{owner}")),
    }
}
async fn project(control: &BillingControl, sub: &Subscription, now: i64) {
    let mut tx = control.pool.begin().await.unwrap();
    control
        .project_subscription(
            &mut tx,
            "stripe",
            &format!("evt-{}-{now}-{}", sub.id, sub.status),
            sub,
            now,
            true,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
async fn period(pool: &PgPool, owner: &str, at: i64) -> Value {
    sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<=$2 AND period_end>$2 ORDER BY period_start DESC LIMIT 1")
        .bind(owner)
        .bind(at)
        .fetch_one(pool)
        .await
        .unwrap()
}
/// One settled usage segment: `awake` running time on a machine of `memory`
/// bytes that cost the provider `cost`.
async fn settle(
    pool: &PgPool,
    owner: &str,
    entry: &str,
    at: i64,
    awake: Option<i64>,
    cost: i64,
    memory: u64,
) {
    let p = period(pool, owner, at).await;
    let resource = json!({"accountId":owner,"kind":"workspace","id":format!("ws-{owner}"),"createdAt":0,"config":{}});
    let mut tx = pool.begin().await.unwrap();
    settle_segment(
        &mut tx,
        &resource,
        UsageIdentity {
            provider: "boxd",
            entry,
            event: entry,
            execution: entry,
            machine: None,
            awake_ms: awake,
        },
        &p,
        at - HOUR,
        at,
        cost,
        4,
        memory,
        at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}
async fn capacity(pool: &PgPool, owner: &str, at: i64) -> Result<(), String> {
    let mut tx = pool.begin().await.unwrap();
    let result = crate::billing::require_capacity(&mut tx, owner, at)
        .await
        .map_err(|e| e.code.to_string());
    tx.rollback().await.unwrap();
    result
}
async fn trial_state(pool: &PgPool, id: &str) -> String {
    sqlx::query_scalar("SELECT state FROM api_cloud_trials WHERE subscription_id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn machine_minute_plans_trials_metering_and_holds() {
    let db = database().await;
    let pool = db.pool.clone();
    for owner in ACCOUNTS {
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('billing',$1,'rust',1,0)")
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
    }
    let control = BillingControl::from_pool(pool.clone(), ACCOUNTS.map(String::from).to_vec());
    let store = Store::from_pool(pool.clone()).with_billing_control(control.clone());
    let t0 = chrono::Utc::now().timestamp_millis() / 1000 * 1000 - 2 * DAY;
    let trial_end = t0 + 7 * DAY;

    // A $1 trial grants 8,400 machine minutes on Cloud machines, no pay as you go.
    project(
        &control,
        &sub(
            "user-trial",
            "sub-trial",
            plans::CLOUD_OFFER,
            true,
            t0,
            trial_end,
        ),
        t0,
    )
    .await;
    let p = period(&pool, "user-trial", t0 + HOUR).await;
    assert_eq!(p["plan_id"], "cloud-trial");
    assert_eq!(p["included_provider_cost_micros"], 5_000_000);
    assert_eq!(p["overage_cap_micros"], 0);
    assert_eq!(trial_state(&pool, "sub-trial").await, "active");
    let summary = store
        .billing_summary("user-trial", t0 + HOUR)
        .await
        .unwrap();
    let plan = summary.plan.clone().unwrap();
    assert!(plan.trial);
    assert_eq!(plan.trial_ends_at, Some(trial_end));
    assert_eq!(plan.included_machine_minutes, 8_400);
    assert!(!plan.pay_as_you_go);
    assert!(capacity(&pool, "user-trial", t0 + HOUR).await.is_ok());
    assert!(size_allowed(Some(plan.plan_id), "large").is_err());

    // Once per card: another account with the same card gets no allowance.
    let mut copycat = sub(
        "user-copycat",
        "sub-copycat",
        plans::CLOUD_OFFER,
        true,
        t0,
        trial_end,
    );
    copycat.card_fingerprint = Some("fp-user-trial".into());
    project(&control, &copycat, t0).await;
    assert_eq!(trial_state(&pool, "sub-copycat").await, "rejected");
    assert_eq!(
        capacity(&pool, "user-copycat", t0 + HOUR)
            .await
            .unwrap_err(),
        "cloud_usage_hold"
    );
    let held = store
        .billing_summary("user-copycat", t0 + HOUR)
        .await
        .unwrap();
    assert_eq!(
        held.plan.unwrap().hold_reason,
        Some(HoldReason::TrialUnavailable)
    );
    // Once per account: a second trial subscription is rejected too.
    let mut again = sub(
        "user-trial",
        "sub-trial-2",
        plans::CLOUD_OFFER,
        true,
        t0 + DAY,
        trial_end + DAY,
    );
    again.card_fingerprint = Some("fp-other-card".into());
    let mut tx = pool.begin().await.unwrap();
    control
        .project_subscription(&mut tx, "stripe", "evt-again", &again, t0, true)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let rejected: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM api_cloud_trials WHERE subscription_id='sub-trial-2' AND state<>'rejected')").fetch_one(&pool).await.unwrap();
    assert!(rejected);

    // Asleep time is free; provider cost is still recorded for margin.
    settle(
        &pool,
        "user-trial",
        "asleep",
        t0 + 2 * HOUR,
        Some(0),
        400_000,
        16 * GIB,
    )
    .await;
    let used: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='machine-time'").bind(p["period_id"].as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(used, 0);
    let cost: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=$1 AND kind='provider-cost'").bind(p["period_id"].as_str()).fetch_one(&pool).await.unwrap();
    assert_eq!(cost, 400_000);
    // 35 awake hours on 4 vCPU / 16 GB use the whole trial: computers pause.
    settle(
        &pool,
        "user-trial",
        "awake-35h",
        t0 + 3 * HOUR,
        Some(35 * HOUR),
        9_000_000,
        16 * GIB,
    )
    .await;
    let p = period(&pool, "user-trial", t0 + 4 * HOUR).await;
    assert_eq!(p["usage_hold"], true);
    assert_eq!(p["hold_reason"], "trial-allowance-used");
    assert_eq!(
        capacity(&pool, "user-trial", t0 + 4 * HOUR)
            .await
            .unwrap_err(),
        "cloud_usage_hold"
    );
    let trial = store
        .billing_summary("user-trial", t0 + 4 * HOUR)
        .await
        .unwrap()
        .plan
        .unwrap();
    assert_eq!(trial.used_machine_minutes, 8_400);
    assert_eq!(trial.remaining_machine_minutes, 0);
    assert_eq!(
        trial.extra_charge_micros, 0,
        "no pay as you go during the trial"
    );
    let charged: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE kind='overage-charge'").fetch_one(&pool).await.unwrap();
    assert_eq!(charged, 0);

    // Day 7: the subscription renews as Cloud with 33,600 machine minutes.
    let mut renewed = sub(
        "user-trial",
        "sub-trial",
        plans::CLOUD_OFFER,
        false,
        trial_end,
        trial_end + 30 * DAY,
    );
    renewed.status = "active".into();
    project(&control, &renewed, trial_end).await;
    assert_eq!(trial_state(&pool, "sub-trial").await, "converted");
    let cloud = period(&pool, "user-trial", trial_end + HOUR).await;
    assert_eq!(cloud["plan_id"], "cloud");
    assert_eq!(cloud["included_provider_cost_micros"], 20_000_000);
    assert_eq!(cloud["overage_cap_micros"], 25_000_000);
    assert_eq!(cloud["usage_hold"], false);
    assert!(
        capacity(&pool, "user-trial", trial_end + HOUR)
            .await
            .is_ok()
    );

    // "Start Cloud now" ends the trial early: the trial period ends where the
    // paid period starts, so periods never overlap.
    project(
        &control,
        &sub(
            "user-early",
            "sub-early",
            plans::CLOUD_OFFER,
            true,
            t0,
            trial_end,
        ),
        t0,
    )
    .await;
    let now = t0 + DAY;
    project(
        &control,
        &sub(
            "user-early",
            "sub-early",
            plans::CLOUD_OFFER,
            false,
            now,
            now + 30 * DAY,
        ),
        now,
    )
    .await;
    let trial_period: i64 = sqlx::query_scalar("SELECT period_end FROM api_cloud_billing_periods WHERE account_id='user-early' AND period_start=$1").bind(t0).fetch_one(&pool).await.unwrap();
    assert_eq!(trial_period, now);
    assert_eq!(
        period(&pool, "user-early", now + HOUR).await["plan_id"],
        "cloud"
    );
    assert_eq!(trial_state(&pool, "sub-early").await, "converted");

    // Cancelled during the trial: no paid period, the trial just ends.
    project(
        &control,
        &sub(
            "user-cancel",
            "sub-cancel",
            plans::CLOUD_OFFER,
            true,
            t0,
            trial_end,
        ),
        t0,
    )
    .await;
    let mut cancelled = sub(
        "user-cancel",
        "sub-cancel",
        plans::CLOUD_OFFER,
        false,
        t0,
        t0 + DAY,
    );
    cancelled.status = "ended".into();
    project(&control, &cancelled, t0 + 2 * DAY).await;
    assert_eq!(trial_state(&pool, "sub-cancel").await, "ended");
    let p = period(&pool, "user-cancel", t0 + HOUR).await;
    assert_eq!(p["plan_id"], "cloud-trial");
    assert_eq!(p["status"], "ended");
    let paid: i64 = sqlx::query_scalar("SELECT count(*) FROM api_cloud_billing_periods WHERE account_id='user-cancel' AND plan_id='cloud'").fetch_one(&pool).await.unwrap();
    assert_eq!(paid, 0);

    // Cloud: past the allowance, extra machine minutes bill at the same rate
    // (no markup) up to the cap, then computers pause.
    project(
        &control,
        &sub(
            "user-cloud",
            "sub-cloud",
            plans::CLOUD_OFFER,
            false,
            t0,
            t0 + 30 * DAY,
        ),
        t0,
    )
    .await;
    settle(
        &pool,
        "user-cloud",
        "cloud-150h",
        t0 + HOUR,
        Some(152 * HOUR),
        30_000_000,
        16 * GIB,
    )
    .await;
    let s = store
        .billing_summary("user-cloud", t0 + 2 * HOUR)
        .await
        .unwrap();
    let plan = s.plan.clone().unwrap();
    assert_eq!(plan.used_machine_minutes, 36_480);
    assert_eq!(plan.extra_machine_minutes, 2_880);
    assert_eq!(plan.extra_charge_micros, 1_714_286);
    assert_eq!(s.markup_basis_points, 0);
    assert_eq!(
        s.provider_cost_micros, 21_714_286,
        "list value, never provider cost"
    );
    let exported: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_cents),0)::bigint FROM api_cloud_billing_outbox WHERE account_id='user-cloud'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        exported, 171,
        "pay as you go is exported in cents to the meter"
    );
    // Lower the cap to just above what is charged, then use one more hour.
    store
        .billing_set_cap(
            "user-cloud",
            BillingCapRequest {
                overage_cap_micros: 1_800_000,
                idempotency_key: "cap-1".into(),
            },
            t0 + 2 * HOUR,
        )
        .await
        .unwrap();
    settle(
        &pool,
        "user-cloud",
        "cloud-1h",
        t0 + 3 * HOUR,
        Some(HOUR),
        200_000,
        16 * GIB,
    )
    .await;
    let held = store
        .billing_summary("user-cloud", t0 + 4 * HOUR)
        .await
        .unwrap();
    let plan = held.plan.clone().unwrap();
    assert!(plan.usage_hold);
    assert_eq!(plan.hold_reason, Some(HoldReason::CapReached));
    assert_eq!(
        plan.extra_charge_micros, 1_800_000,
        "never charged past the cap"
    );
    assert_eq!(
        capacity(&pool, "user-cloud", t0 + 4 * HOUR)
            .await
            .unwrap_err(),
        "cloud_usage_hold"
    );
    // Raising the cap resumes.
    store
        .billing_set_cap(
            "user-cloud",
            BillingCapRequest {
                overage_cap_micros: 25_000_000,
                idempotency_key: "cap-2".into(),
            },
            t0 + 4 * HOUR,
        )
        .await
        .unwrap();
    assert!(capacity(&pool, "user-cloud", t0 + 4 * HOUR).await.is_ok());
    // Nothing a person sees names the provider.
    let usage = store
        .billing_usage("user-cloud", None, 50, t0 + 4 * HOUR)
        .await
        .unwrap();
    assert!(usage.items.iter().all(|i| i.provider == "cloud"));
    let item = usage
        .items
        .iter()
        .find(|i| i.entry_id == "cloud-150h")
        .unwrap();
    assert_eq!(item.machine_rate, Some(4));
    assert_eq!(item.machine_minutes, Some(36_480));
    // Machine-image builds settle under their own kind; the page still loads
    // and shows them as image builds instead of rejecting every row.
    sqlx::query("UPDATE api_cloud_billing_usage SET resource_kind='machine-image' WHERE entry_id='cloud-150h'")
        .execute(&pool)
        .await
        .unwrap();
    let with_image = store
        .billing_usage("user-cloud", None, 50, t0 + 4 * HOUR)
        .await
        .unwrap();
    let image = with_image
        .items
        .iter()
        .find(|i| i.entry_id == "cloud-150h")
        .unwrap();
    assert_eq!(image.resource_kind, "build");
    sqlx::query(
        "UPDATE api_cloud_billing_usage SET resource_kind='workspace' WHERE entry_id='cloud-150h'",
    )
    .execute(&pool)
    .await
    .unwrap();
    for text in [
        serde_json::to_string(&held).unwrap(),
        serde_json::to_string(&usage).unwrap(),
    ] {
        assert!(!names_infrastructure(&text), "{text}");
    }

    // Cloud Max: larger machines use machine minutes twice as fast.
    project(
        &control,
        &sub(
            "user-max",
            "sub-max",
            plans::CLOUD_MAX_OFFER,
            false,
            t0,
            t0 + 30 * DAY,
        ),
        t0,
    )
    .await;
    settle(
        &pool,
        "user-max",
        "max-1h",
        t0 + HOUR,
        Some(HOUR),
        300_000,
        32 * GIB,
    )
    .await;
    let max = store
        .billing_summary("user-max", t0 + 2 * HOUR)
        .await
        .unwrap()
        .plan
        .unwrap();
    assert_eq!(max.included_machine_minutes, 84_000);
    assert_eq!(max.used_machine_minutes, 480);
    assert!(size_allowed(Some(max.plan_id), "large").is_ok());
    assert_eq!(
        current_plan(&pool, "user-max", t0 + HOUR).await.unwrap(),
        Some(PlanId::CloudMax)
    );

    // The original $40 subscription gets Cloud's allowance; a period already
    // on original terms keeps them until it renews.
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,provider_subscription_id,billing_provider,status,currency,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,price_catalog_version,overage_cap_micros,created_at,updated_at) VALUES('legacy-now','user-max','sub-legacy','stripe','active','USD',$1,$2,40000000,35000000,500,'provider-compute-v1',25000000,0,0)").bind(t0 + 40 * DAY).bind(t0 + 70 * DAY).execute(&pool).await.unwrap();
    project(
        &control,
        &sub(
            "user-max",
            "sub-legacy",
            plans::LEGACY_OFFER,
            false,
            t0 + 40 * DAY,
            t0 + 70 * DAY,
        ),
        t0 + 40 * DAY,
    )
    .await;
    let kept = period(&pool, "user-max", t0 + 41 * DAY).await;
    assert_eq!(kept["price_catalog_version"], "provider-compute-v1");
    assert!(plan_usage(&kept, 0, 0).is_none());
    project(
        &control,
        &sub(
            "user-max",
            "sub-legacy",
            plans::LEGACY_OFFER,
            false,
            t0 + 70 * DAY,
            t0 + 100 * DAY,
        ),
        t0 + 70 * DAY,
    )
    .await;
    let legacy = period(&pool, "user-max", t0 + 71 * DAY).await;
    assert_eq!(legacy["plan_id"], "legacy");
    assert_eq!(legacy["base_price_micros"], 40_000_000);
    let usage = plan_usage(&legacy, 0, 0).unwrap();
    assert_eq!(usage.included_machine_minutes, 33_600);
    assert_eq!(usage.plan_name, "Cloud");

    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {} CASCADE", db.schema))
        .execute(&db.admin)
        .await
        .unwrap();
}
