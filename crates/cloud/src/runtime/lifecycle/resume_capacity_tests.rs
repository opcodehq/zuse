//! Waking, resuming, recreating, repairing and restoring a computer all admit
//! through `require_resume_capacity`. It never consults an account allowlist,
//! so the entitled access policy relies on it alone.
use super::*;

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn resume_requires_paid_entitlement_and_billing_capacity() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(url.contains("127.0.0.1") || url.contains("localhost"));
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("resume_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |connection, _| {
            let schema = copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE api_entitlements(entitlement_id text PRIMARY KEY,account_id text,kind text,status text,paid_through bigint); CREATE TABLE api_cloud_billing_periods(period_id text PRIMARY KEY,account_id text,status text,currency text,period_start bigint,period_end bigint,included_provider_cost_micros bigint,markup_basis_points bigint,overage_cap_micros bigint); CREATE TABLE api_cloud_billing_ledger(period_id text,kind text,amount_micros bigint); CREATE TABLE api_cloud_billing_reservations(period_id text,provider_cost_micros bigint,expires_at bigint);")
        .execute(&pool)
        .await
        .unwrap();
    let now = 1_000_000;
    // An unlisted account: nothing about the allowlists is consulted.
    let row =
        serde_json::json!({"account_id":"user-unlisted","workspace_id":"ws","request_config":{}});
    let check = || async {
        let mut tx = pool.begin().await.unwrap();
        let result = require_resume_capacity(None, &mut tx, &row, now).await;
        tx.rollback().await.unwrap();
        result.err().map(|e| e.code)
    };
    assert_eq!(check().await, Some("cloud_entitlement_required"));
    sqlx::query(
        "INSERT INTO api_entitlements VALUES('ent','user-unlisted','cloud-workspace','ended',$1)",
    )
    .bind(now - 1)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(check().await, Some("cloud_entitlement_required"));
    sqlx::query("UPDATE api_entitlements SET status='active',paid_through=$1")
        .bind(now + 1000)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(check().await, Some("cloud_billing_period_missing"));
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('p','user-unlisted','active','USD',0,$1,1000,0,0)")
        .bind(now + 1000)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(check().await, None);
    sqlx::query("INSERT INTO api_cloud_billing_ledger VALUES('p','provider-cost',1001)")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(check().await, Some("cloud_billing_hold"));
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
