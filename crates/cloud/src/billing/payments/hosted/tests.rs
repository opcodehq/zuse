use super::*;
use axum::http::StatusCode;
use sqlx::{PgPool, postgres::PgPoolOptions};

async fn usage(pool: &PgPool, request: &str) -> (Option<i64>, bool) {
    sqlx::query_as("SELECT cost_micros,estimated FROM zuse_rust_model_usage WHERE request_id=$1")
        .bind(request)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn hosted_usage_spends_wallet_credit_exactly_once() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("Explicit isolated PostgreSQL required");
    assert!(matches!(
        Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("hosted_models_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |conn, _| {
            let schema = copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(crate::jobs::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    sqlx::raw_sql(crate::billing::WALLET_MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO zuse_rust_control_owners VALUES('billing','user-alice','rust',1,0)")
        .execute(&pool)
        .await
        .unwrap();
    let separator = if url.contains('?') { '&' } else { '?' };
    let scoped = format!("{url}{separator}options=-csearch_path%3D{schema}");
    let wallet = crate::billing::Wallet::fixture(&scoped).await;
    wallet.fund("user-alice", "5").await;
    let control =
        BillingControl::from_pool(pool.clone(), vec!["user-alice".into()]).with_wallet(wallet);
    let now = chrono::Utc::now().timestamp_millis();
    let spendable = || control.hosted_spendable_micros("user-alice");

    assert_eq!(spendable().await.unwrap(), 5_000_000);
    // Accounts without Rust billing ownership cannot open requests.
    assert!(
        control
            .open_model_request("user-bob", "r-bob", "m", 1, 1, now)
            .await
            .is_err()
    );

    // An open request holds credit, and closing it spends only what it cost.
    control
        .open_model_request("user-alice", "r1", "m", 30_000, 50_000, now)
        .await
        .unwrap();
    assert_eq!(spendable().await.unwrap(), 4_950_000);
    control
        .close_model_request("r1", (1000, 500), 25_000, false, now)
        .await
        .unwrap();
    assert_eq!(spendable().await.unwrap(), 4_975_000);
    // A late duplicate close never rewrites or respends settled usage.
    control
        .close_model_request("r1", (1, 1), 1, false, now)
        .await
        .unwrap();
    assert_eq!(usage(&pool, "r1").await, (Some(25_000), false));
    assert_eq!(spendable().await.unwrap(), 4_975_000);

    // A settlement whose acknowledgement was lost replays its key without spending again.
    sqlx::query("UPDATE zuse_rust_model_usage SET settled_at=NULL WHERE request_id='r1'")
        .execute(&pool)
        .await
        .unwrap();
    let report = control.settle_hosted_models_once(now).await.unwrap();
    assert_eq!((report.settled, report.deferred), (1, 0));
    assert_eq!(spendable().await.unwrap(), 4_975_000);
    assert_eq!(
        control
            .settle_hosted_models_once(now)
            .await
            .unwrap()
            .settled,
        0
    );

    // A request that outgrew its hold still pays its real cost.
    control
        .open_model_request("user-alice", "r2", "m", 1_000, 10_000, now)
        .await
        .unwrap();
    control
        .close_model_request("r2", (1, 1), 20_000, false, now)
        .await
        .unwrap();
    assert_eq!(spendable().await.unwrap(), 4_955_000);

    // A request whose stream was lost bills its input estimate, not its whole hold.
    control
        .open_model_request("user-alice", "r3", "m", 30_000, 80_000, now - 3_700_000)
        .await
        .unwrap();
    assert_eq!(spendable().await.unwrap(), 4_875_000);
    let report = control.settle_hosted_models_once(now).await.unwrap();
    assert_eq!((report.abandoned, report.settled), (1, 1));
    assert_eq!(usage(&pool, "r3").await, (Some(30_000), true));
    assert_eq!(spendable().await.unwrap(), 4_925_000);

    // A hold larger than the balance is refused before any upstream call.
    let refused = control
        .open_model_request("user-alice", "r4", "m", 1, 99_000_000, now)
        .await
        .unwrap_err();
    assert_eq!(refused.status, StatusCode::PAYMENT_REQUIRED);
    assert_eq!(refused.code, "hosted_models_insufficient_credits");
    assert_eq!(spendable().await.unwrap(), 4_925_000);

    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
