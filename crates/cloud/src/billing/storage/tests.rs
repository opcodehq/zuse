use super::*;
#[test]
fn original_schedule_keeps_fractional_micros_across_checkpoints() {
    let (mut total, mut remainder) = (0, 0);
    for _ in 0..720 {
        let (cost, r) = storage_cost(3_600_000, remainder, MONTH_MICROS).unwrap();
        total += cost;
        remainder = r;
    }
    assert_eq!((total, remainder), (MONTH_MICROS, 0));
    let (a, r) = storage_cost(1, 0, MONTH_MICROS).unwrap();
    let (b, r) = storage_cost(MONTH_MS - 1, r, MONTH_MICROS).unwrap();
    assert_eq!((a + b, r), (MONTH_MICROS, 0));
    for (d, r, rate) in [
        (-1, 0, MONTH_MICROS),
        (1, -1, MONTH_MICROS),
        (1, MONTH_MS, MONTH_MICROS),
        (1, 0, -1),
        (MAX, 0, MAX),
    ] {
        assert!(storage_cost(d, r, rate).is_err());
    }
}
async fn snapshot(
    pool: &sqlx::PgPool,
    id: &str,
    account: &str,
    state: &str,
    start: i64,
    stop: Option<i64>,
) {
    let record = json!({"snapshotId":id,"provider":"box","accountId":account,"buildId":format!("build-{account}"),"state":state,"createdAtMs":start,"retainedAtMs":start,"checkpointAtMs":start,"stoppedAtMs":stop,"remainder":0,"attempts":2,"nextAttemptAtMs":123,"lastError":"preserved-reference-field","extra":{"preserve":true}});
    sqlx::query("INSERT INTO api_cloud_snapshots VALUES('box',$1,$2,$3,$4,$5,$6)")
        .bind(id)
        .bind(account)
        .bind(format!("build-{account}"))
        .bind(state)
        .bind(start)
        .bind(record)
        .execute(pool)
        .await
        .unwrap();
}
async fn checkpoint(pool: &sqlx::PgPool, id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT (record->>'checkpointAtMs')::bigint FROM api_cloud_snapshots WHERE snapshot_id=$1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn retained_images_settle_exactly_with_checkpoints_leases_deleted_legacy_and_outboxes() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("storage_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
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
    for sql in [
        include_str!("../../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../../tests/fixtures/settlement.sql"),
        include_str!("../../../tests/fixtures/snapshot-storage.sql"),
        crate::jobs::MIGRATION,
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    sqlx::query("ALTER TABLE api_provider_usage_events ADD COLUMN occurred_at bigint")
        .execute(&pool)
        .await
        .unwrap();
    let start = 1700006400000;
    let middle = start + MONTH_MS / 2;
    let end = start + MONTH_MS;
    for (id, account, begin, finish) in [
        ("p1", "alice", start, middle),
        ("p2", "alice", middle, end),
        ("legacy", "legacy", start, end),
        ("byok", "byok", start, end),
    ] {
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,billing_provider,provider_subscription_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,overage_cap_micros,created_at,updated_at) VALUES($1,$2,'stripe','sub-original','active',$3,$4,40000000,35000000,500,25000000,$3,$3)").bind(id).bind(account).bind(begin).bind(finish).execute(&pool).await.unwrap();
    }
    for account in ["alice", "legacy", "byok"] {
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('billing',$1,'rust',1,0)")
            .bind(account)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (account, config) in [
        ("alice", json!({})),
        ("legacy", json!({})),
        ("byok", json!({"providerConnectionId":"customer-owned"})),
    ] {
        sqlx::query("INSERT INTO api_cloud_project_builds VALUES($1,$2,'box',NULL,$3,$4)")
            .bind(format!("build-{account}"))
            .bind(account)
            .bind(config)
            .bind(start)
            .execute(&pool)
            .await
            .unwrap();
    }
    snapshot(&pool, "image-alice", "alice", "retained", start, None).await;
    snapshot(
        &pool,
        "image-legacy",
        "legacy",
        "deleted",
        start,
        Some(middle),
    )
    .await;
    snapshot(&pool, "image-byok", "byok", "retained", start, None).await;
    snapshot(&pool, "image-unowned", "unowned", "retained", start, None).await;
    snapshot(&pool, "image-foreign", "foreign", "retained", start, None).await;
    let legacy = format!("snapshot:image-legacy:{VERSION}:{start}:{middle}");
    sqlx::query("INSERT INTO api_provider_event_finalizations VALUES('box','original-legacy-event',$1,$2,$3)").bind(&legacy).bind(middle).bind(end+DAY).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_usage VALUES($1,'legacy','legacy','snapshot','image-legacy','box',$2,$2,$3,$4,0,0,850000,'confirmed',$4)").bind(format!("box:{legacy}")).bind(&legacy).bind(start).bind(middle).execute(&pool).await.unwrap();
    for (id, p, account, cost) in [
        ("base1", "p1", "alice", 34_500_000),
        ("base2", "p2", "alice", 35_000_000),
        ("legacy-source", "legacy", "legacy", 850_000),
    ] {
        sqlx::query("INSERT INTO api_cloud_billing_ledger VALUES($1,$2,$3,'provider-cost',$4,$1,'{}',$5,$5)").bind(id).bind(p).bind(account).bind(cost as i64).bind(start).execute(&pool).await.unwrap();
    }
    let control = BillingControl::from_pool(
        pool.clone(),
        vec![
            "alice".into(),
            "legacy".into(),
            "byok".into(),
            "unowned".into(),
        ],
    );
    let config = SnapshotSettlementConfig {
        cutover_at_ms: start,
    };
    let first = control
        .settle_snapshot_storage_once(config, middle)
        .await
        .unwrap();
    assert_eq!(first.settled, 1);
    assert_eq!(first.deferred, 1);
    assert!(first.errors[0].contains("cloud_billing_owner_not_rust"));
    assert_eq!(checkpoint(&pool, "image-alice").await, middle);
    assert_eq!(checkpoint(&pool, "image-legacy").await, middle);
    assert_eq!(checkpoint(&pool, "image-byok").await, start);
    assert_eq!(checkpoint(&pool, "image-unowned").await, start);
    assert_eq!(checkpoint(&pool, "image-foreign").await, start);
    let interval = control
        .settle_snapshot_storage_once(config, middle + 30 * 60_000)
        .await
        .unwrap();
    assert_eq!(interval.settled, 0);
    assert_eq!(checkpoint(&pool, "image-alice").await, middle);
    // Shared image consumption blocks checkpoint writes just like exclusive promotion/deletion.
    sqlx::query("INSERT INTO api_cloud_snapshot_leases VALUES('alice','consumer','shared',$1)")
        .bind(chrono::Utc::now().timestamp_millis() + 3_600_000)
        .execute(&pool)
        .await
        .unwrap();
    let busy = control
        .settle_snapshot_storage_once(config, end)
        .await
        .unwrap();
    assert!(busy.errors.iter().any(|e| e == "alice:cloud_snapshot_busy"));
    assert_eq!(checkpoint(&pool, "image-alice").await, middle);
    sqlx::query("DELETE FROM api_cloud_snapshot_leases WHERE owner='consumer'")
        .execute(&pool)
        .await
        .unwrap();
    // A failed checkpoint must roll back the new usage, ledger, outbox and finalization.
    sqlx::raw_sql("CREATE FUNCTION fail_checkpoint() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.snapshot_id='image-alice' THEN RAISE EXCEPTION 'fixture checkpoint interrupted'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_checkpoint BEFORE UPDATE ON api_cloud_snapshots FOR EACH ROW EXECUTE FUNCTION fail_checkpoint()").execute(&pool).await.unwrap();
    let failed = control
        .settle_snapshot_storage_once(config, end)
        .await
        .unwrap();
    assert!(failed.errors.iter().any(|e| e.starts_with("alice:")));
    assert_eq!(checkpoint(&pool, "image-alice").await, middle);
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM api_cloud_billing_usage WHERE account_id='alice'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    sqlx::query("DROP TRIGGER fail_checkpoint ON api_cloud_snapshots")
        .execute(&pool)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        control.settle_snapshot_storage_once(config, end),
        control.settle_snapshot_storage_once(config, end)
    );
    assert_eq!(a.unwrap().settled + b.unwrap().settled, 1);
    assert_eq!(checkpoint(&pool, "image-alice").await, end);
    let costs:Vec<(String,i64)>=sqlx::query_as("SELECT period_id,SUM(amount_cents)::bigint FROM api_cloud_billing_outbox GROUP BY period_id ORDER BY period_id").fetch_all(&pool).await.unwrap();
    assert_eq!(costs, vec![("p1".into(), 37), ("p2".into(), 89)]);
    let units:i64=sqlx::query_scalar("SELECT SUM(provider_cost_micros)::bigint FROM api_cloud_billing_usage WHERE account_id='alice'").fetch_one(&pool).await.unwrap();
    assert_eq!(units, MONTH_MICROS);
    let telemetry:i64=sqlx::query_scalar("SELECT COUNT(*) FROM api_cloud_usage_outbox WHERE payload->'metadata'->>'cost_source'='approved-storage-schedule' AND payload->'metadata'->>'measurement'='confirmed' AND payload->'metadata'->>'billable'='false'").fetch_one(&pool).await.unwrap();
    assert_eq!(telemetry, 2);
    let remainder: Value = sqlx::query_scalar(
        "SELECT record FROM api_cloud_snapshots WHERE snapshot_id='image-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remainder["remainder"], 0);
    assert_eq!(remainder["extra"]["preserve"], true);
    assert_eq!(remainder["attempts"], 2);
    assert_eq!(remainder["lastError"], "preserved-reference-field");
    assert_eq!(
        control
            .settle_snapshot_storage_once(config, end)
            .await
            .unwrap()
            .settled,
        0
    );
    // Late final settlement for an already deleted image includes the final partial hour.
    let partial_start = end - 3_600_001;
    snapshot(
        &pool,
        "image-deleted",
        "alice",
        "deleted",
        partial_start,
        Some(end),
    )
    .await;
    let final_partial = control
        .settle_snapshot_storage_once(config, end)
        .await
        .unwrap();
    assert_eq!(final_partial.settled, 1);
    assert_eq!(checkpoint(&pool, "image-deleted").await, end);
    let amount:i64=sqlx::query_scalar("SELECT provider_cost_micros FROM api_cloud_billing_usage WHERE resource_id='image-deleted'").fetch_one(&pool).await.unwrap();
    assert_eq!(amount, storage_cost(3_600_001, 0, MONTH_MICROS).unwrap().0);
    assert!(
        sqlx::query("UPDATE api_snapshot_price_schedule SET monthly_micros=1")
            .execute(&pool)
            .await
            .is_err()
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
