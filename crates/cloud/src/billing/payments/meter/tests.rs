use super::*;
use crate::billing::{BillingControl, MeterExportConfig};
use axum::{
    Json, Router,
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    },
};
struct Provider {
    start: i64,
    end: i64,
    mode: AtomicUsize,
    fail_post: AtomicBool,
    fail_summary: AtomicBool,
    posts: Mutex<HashMap<String, (String, i64)>>,
    post_calls: AtomicUsize,
    summary_calls: AtomicUsize,
    charge: AtomicI64,
}
fn auth(h: &HeaderMap) {
    assert_eq!(h["authorization"], "Bearer fixture-key");
}
async fn customer(h: HeaderMap) -> Json<Value> {
    auth(&h);
    Json(json!({"id":"cus_original","metadata":{"account_id":"user-alice"}}))
}
async fn subscription(State(f): State<Arc<Provider>>, h: HeaderMap) -> Json<Value> {
    auth(&h);
    Json(
        json!({"id":"sub-preserved","customer":"cus_original","status":if f.mode.load(Ordering::SeqCst)==0{"active"}else{"canceled"},"items":{"data":[{"price":{"id":"price_meter"},"current_period_start":f.start/1000,"current_period_end":f.end/1000}]}}),
    )
}
async fn invoices(
    State(f): State<Arc<Provider>>,
    Query(q): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Json<Value> {
    auth(&h);
    assert_eq!(q["subscription"], "sub-preserved");
    assert_eq!(q["created[gte]"], (f.end / 1000 - 300).to_string());
    Json(
        json!({"data":[{"id":"in_original","status":if f.mode.load(Ordering::SeqCst)==1{"draft"}else{"open"}}],"has_more":false}),
    )
}
async fn lines(State(f): State<Arc<Provider>>, h: HeaderMap) -> Json<Value> {
    auth(&h);
    Json(
        json!({"data":[{"id":"il_original","pricing":{"price_details":{"price":"price_meter"}},"period":{"start":f.start/1000,"end":f.end/1000}}],"has_more":false}),
    )
}
async fn meter(
    State(f): State<Arc<Provider>>,
    h: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    auth(&h);
    f.post_calls.fetch_add(1, Ordering::SeqCst);
    let key = h["idempotency-key"].to_str().unwrap().to_owned();
    assert_eq!(form["identifier"], key);
    assert_eq!(form["payload[stripe_customer_id]"], "cus_original");
    let units = form["payload[value]"].parse::<i64>().unwrap();
    let name = form["event_name"].clone();
    let mut posts = f.posts.lock().unwrap();
    if let Some(old) = posts.get(&key) {
        assert_eq!(old, &(name.clone(), units));
    } else {
        posts.insert(key, (name.clone(), units));
        if name == "zuse_cloud_overage_cent" {
            f.charge.fetch_add(units, Ordering::SeqCst);
        }
    }
    if f.fail_post.swap(false, Ordering::SeqCst) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(Json(json!({"object":"billing.meter_event"})))
}
async fn summaries(
    State(f): State<Arc<Provider>>,
    Query(q): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    auth(&h);
    f.summary_calls.fetch_add(1, Ordering::SeqCst);
    assert_eq!(q["customer"], "cus_original");
    assert_eq!(
        q["start_time"],
        (((f.start + 59999) / 60000) * 60).to_string()
    );
    assert_eq!(q["end_time"], (((f.end + 59999) / 60000) * 60).to_string());
    if f.fail_summary.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if q.contains_key("starting_after") {
        assert_eq!(q["starting_after"], "sum-one");
        Ok(Json(
            json!({"data":[{"id":"sum-two","aggregated_value":23.0}],"has_more":false}),
        ))
    } else {
        Ok(Json(
            json!({"data":[{"id":"sum-one","aggregated_value":100.0}],"has_more":true}),
        ))
    }
}
fn config(financial: bool, telemetry: bool) -> MeterExportConfig {
    MeterExportConfig {
        financial,
        telemetry,
        meters: BTreeMap::from([("stripe".into(), "meter-stripe".into())]),
    }
}
fn event(owner: &str, key: &str, name: &str, units: i64, now: i64) -> Value {
    json!({"accountId":owner,"eventId":key,"eventName":name,"units":units,"occurredAtMs":now,"billingProvider":"stripe","metadata":{"billable":"false"}})
}
async fn usage(pool: &PgPool, key: &str, value: Value, now: i64) {
    sqlx::query("INSERT INTO api_cloud_usage_outbox(event_id,payload,created_at,next_attempt_at) VALUES($1,$2,$3,$3)").bind(key).bind(value).bind(now).execute(pool).await.unwrap();
}
async fn financial(
    pool: &PgPool,
    key: &str,
    owner: &str,
    period: &str,
    provider: &str,
    units: i64,
    now: i64,
) {
    sqlx::query("INSERT INTO api_cloud_billing_outbox(outbox_id,period_id,account_id,provider,amount_cents,idempotency_key,attempt_count,next_attempt_at,created_at,occurred_at) VALUES($1,$2,$3,$4,$5,$1,0,$6,$6,$6)").bind(key).bind(period).bind(owner).bind(provider).bind(units).bind(now).execute(pool).await.unwrap();
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn original_outboxes_export_once_recover_receipts_and_reconcile() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("Explicit isolated PostgreSQL required");
    assert!(matches!(
        Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("billing_exports_{}", uuid::Uuid::new_v4().simple());
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
    sqlx::raw_sql(include_str!(
        "../../../../tests/fixtures/legacy-catalog.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../../../../tests/fixtures/billing.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(crate::jobs::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE api_stripe_customers(account_id text PRIMARY KEY,customer_id text UNIQUE); INSERT INTO api_stripe_customers VALUES('user-alice','cus_original'); CREATE TABLE api_stripe_meter_deliveries(delivery_key text PRIMARY KEY,payload text NOT NULL,first_attempt_at bigint NOT NULL,lease_until bigint NOT NULL DEFAULT 0,sent_at bigint); CREATE TABLE api_cloud_billing_outbox(outbox_id text PRIMARY KEY,period_id text NOT NULL,account_id text NOT NULL,provider text NOT NULL,amount_cents bigint NOT NULL,idempotency_key text NOT NULL,attempt_count bigint NOT NULL DEFAULT 0,next_attempt_at bigint NOT NULL,created_at bigint NOT NULL,occurred_at bigint,acknowledged_at bigint,last_error text,UNIQUE(provider,idempotency_key)); CREATE TABLE api_cloud_usage_outbox(event_id text PRIMARY KEY,payload jsonb NOT NULL,created_at bigint NOT NULL,next_attempt_at bigint NOT NULL,attempt_count bigint NOT NULL DEFAULT 0,acknowledged_at bigint,last_error text); ALTER TABLE api_cloud_billing_meter_reconciliations ADD COLUMN reconciliation_id text PRIMARY KEY; ALTER TABLE api_cloud_billing_meter_reconciliations ADD COLUMN expected_units bigint; ALTER TABLE api_cloud_billing_meter_reconciliations ADD COLUMN observed_units bigint; CREATE TABLE api_cloud_billing_meter_reconciliation_attempts(period_id text,provider text,attempted_at bigint NOT NULL,PRIMARY KEY(period_id,provider)); INSERT INTO zuse_rust_control_owners VALUES('billing','user-alice','rust',1,0),('billing','user-bob','rust',1,0)").execute(&pool).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis() / 1000 * 1000;
    let start = now - 3600000;
    let end = now + 86400000;
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('p-alice','user-alice','stripe','sub-preserved','active','USD',$1,$2,40000000,35000000,500,31000000,$1)").bind(start).bind(end).execute(&pool).await.unwrap();
    financial(
        &pool,
        "legacy-stripe-key",
        "user-alice",
        "p-alice",
        "stripe",
        123,
        now,
    )
    .await;
    let f = Arc::new(Provider {
        start,
        end,
        mode: AtomicUsize::new(0),
        fail_post: AtomicBool::new(true),
        fail_summary: AtomicBool::new(false),
        posts: Mutex::new(HashMap::new()),
        post_calls: AtomicUsize::new(0),
        summary_calls: AtomicUsize::new(0),
        charge: AtomicI64::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/customers/cus_original", get(customer))
                .route("/subscriptions/sub-preserved", get(subscription))
                .route("/invoices", get(invoices))
                .route("/invoices/in_original/lines", get(lines))
                .route("/billing/meter_events", post(meter))
                .route(
                    "/billing/meters/meter-stripe/event_summaries",
                    get(summaries),
                )
                .with_state(f.clone()),
        )
        .into_future(),
    );
    let http = || ProviderHttp {
        client: Client::new(),
        origin: Url::parse(&origin).unwrap(),
        secret: Zeroizing::new("fixture-key".into()),
    };
    let stripe = Arc::new(StripePortal {
        http: http(),
        return_url: "https://api-rust.zuse.sh".into(),
        configuration: None,
        meter_price: Some("price_meter".into()),
        credit_price: None,
        events: None,
        checkout: None,
    });
    let providers = PaymentProviders::new("stripe".into(), vec![stripe]).unwrap();
    let base =
        BillingControl::from_pool(pool.clone(), vec!["user-alice".into(), "user-bob".into()])
            .with_payment_providers(providers);
    let control = base.clone().with_meter_exports(config(true, false));
    assert!(!base.export_once(now, 25).await.unwrap().enabled);
    assert_eq!(control.export_once(now, 25).await.unwrap().financial, 0);
    assert_eq!(f.post_calls.load(Ordering::SeqCst), 0);
    sqlx::raw_sql("INSERT INTO zuse_rust_control_owners VALUES('billing-export','user-alice','rust',1,0),('billing-export','user-bob','rust',1,0)").execute(&pool).await.unwrap();
    let result = control.export_once(now, 25).await.unwrap();
    assert_eq!(result.financial, 0);
    assert_eq!(result.retried, 1);
    assert_eq!(f.charge.load(Ordering::SeqCst), 123);
    assert_eq!(
        control
            .export_once(now + 60001, 25)
            .await
            .unwrap()
            .financial,
        1
    );
    assert_eq!(f.charge.load(Ordering::SeqCst), 123);
    assert_eq!(f.post_calls.load(Ordering::SeqCst), 2);
    // Losing the local acknowledgment after a durable provider receipt cannot resend on a closed invoice.
    sqlx::query("UPDATE api_cloud_billing_outbox SET acknowledged_at=NULL,next_attempt_at=0 WHERE outbox_id='legacy-stripe-key'").execute(&pool).await.unwrap();
    f.mode.store(2, Ordering::SeqCst);
    assert_eq!(
        control
            .export_once(now + 60002, 25)
            .await
            .unwrap()
            .financial,
        1
    );
    assert_eq!(f.post_calls.load(Ordering::SeqCst), 2);
    sqlx::query("UPDATE api_cloud_billing_outbox SET acknowledged_at=$1")
        .bind(now - 600000)
        .execute(&pool)
        .await
        .unwrap();
    let mut missing = config(false, true);
    missing.meters.remove("stripe");
    let telemetry = base.clone().with_meter_exports(missing);
    assert_eq!(
        telemetry
            .export_once(now + 60003, 1)
            .await
            .unwrap()
            .reconciled,
        0
    );
    assert_eq!(f.summary_calls.load(Ordering::SeqCst), 0);
    sqlx::query("UPDATE api_cloud_billing_meter_reconciliation_attempts SET attempted_at=0 WHERE provider='stripe'").execute(&pool).await.unwrap();
    f.fail_summary.store(true, Ordering::SeqCst);
    let telemetry = base.clone().with_meter_exports(config(false, true));
    assert_eq!(
        telemetry
            .export_once(now + 60005, 1)
            .await
            .unwrap()
            .reconciled,
        0
    );
    let calls = f.summary_calls.load(Ordering::SeqCst);
    assert_eq!(
        telemetry
            .export_once(now + 60006, 1)
            .await
            .unwrap()
            .reconciled,
        0
    );
    assert_eq!(f.summary_calls.load(Ordering::SeqCst), calls);
    sqlx::query("UPDATE api_cloud_billing_meter_reconciliation_attempts SET attempted_at=0 WHERE provider='stripe'").execute(&pool).await.unwrap();
    f.fail_summary.store(false, Ordering::SeqCst);
    let (a, b) = tokio::join!(
        telemetry.export_once(now + 60007, 25),
        telemetry.export_once(now + 60007, 25)
    );
    assert_eq!(a.unwrap().reconciled + b.unwrap().reconciled, 1);
    let observed:(i64,i64)=sqlx::query_as("SELECT expected_units,observed_units FROM api_cloud_billing_meter_reconciliations WHERE provider='stripe' ORDER BY created_at DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(observed, (123, 123));
    // Telemetry cannot be used to smuggle invoice units through the observation queue.
    usage(
        &pool,
        "runtime-good",
        event(
            "user-alice",
            "runtime-good",
            "zuse_cloud_runtime_observed_ms",
            1000,
            now,
        ),
        now,
    )
    .await;
    usage(
        &pool,
        "runtime-bad",
        event(
            "user-alice",
            "runtime-bad",
            "zuse_cloud_overage_cent",
            999999,
            now,
        ),
        now,
    )
    .await;
    usage(
        &pool,
        "cost-observation",
        event(
            "user-alice",
            "cost-observation",
            "zuse_cloud_provider_cost_micros",
            5000,
            now,
        ),
        now,
    )
    .await;
    let result = telemetry.export_once(now + 60008, 25).await.unwrap();
    assert_eq!(result.telemetry, 2);
    assert_eq!(result.retried, 1);
    assert_eq!(f.charge.load(Ordering::SeqCst), 123);
    // A source period can admit a historical draft invoice, while a finalized invoice stays retryable.
    financial(
        &pool,
        "closed-period-key",
        "user-alice",
        "p-alice",
        "stripe",
        5,
        now,
    )
    .await;
    let result = control.export_once(now + 60009, 25).await.unwrap();
    assert_eq!(result.retried, 1);
    assert_eq!(f.charge.load(Ordering::SeqCst), 123);
    f.mode.store(1, Ordering::SeqCst);
    sqlx::query(
        "UPDATE api_cloud_billing_outbox SET next_attempt_at=0 WHERE outbox_id='closed-period-key'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        control
            .export_once(now + 60010, 25)
            .await
            .unwrap()
            .financial,
        1
    );
    assert_eq!(f.charge.load(Ordering::SeqCst), 128);
    // An uncertain delivery older than Stripe's retry window must not emit another event.
    financial(&pool, "aged-key", "user-alice", "p-alice", "stripe", 7, now).await;
    let event = MeterEvent {
        account_id: "user-alice".into(),
        event_name: "zuse_cloud_overage_cent".into(),
        units: 7,
        idempotency_key: "aged-key".into(),
        occurred_at_ms: now,
        metadata: BTreeMap::new(),
    };
    let (key, payload) =
        crate::billing::payments::stripe_meter_command(&event, "cus_original", now).unwrap();
    sqlx::query("INSERT INTO api_stripe_meter_deliveries VALUES($1,$2,$3,0,NULL)")
        .bind(key)
        .bind(payload)
        .bind(now - 24 * 3600000)
        .execute(&pool)
        .await
        .unwrap();
    let calls = f.post_calls.load(Ordering::SeqCst);
    assert_eq!(
        control.export_once(now + 60011, 25).await.unwrap().retried,
        1
    );
    assert_eq!(f.post_calls.load(Ordering::SeqCst), calls);
    let error: String = sqlx::query_scalar(
        "SELECT last_error FROM api_cloud_billing_outbox WHERE outbox_id='aged-key'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(error, "billing_meter_reconciliation_required");
    // Removing export ownership leaves accepted work for its original exporter without mutating it.
    sqlx::query(
        "UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_kind='billing-export'",
    )
    .execute(&pool)
    .await
    .unwrap();
    financial(
        &pool,
        "not-owned",
        "user-alice",
        "p-alice",
        "stripe",
        10,
        now,
    )
    .await;
    assert_eq!(
        control
            .export_once(now + 60012, 25)
            .await
            .unwrap()
            .financial,
        0
    );
    let attempts: i64 = sqlx::query_scalar(
        "SELECT attempt_count FROM api_cloud_billing_outbox WHERE outbox_id='not-owned'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 0);
    let unchanged: String = sqlx::query_scalar(
        "SELECT provider_subscription_id FROM api_entitlements WHERE entitlement_id='ent-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unchanged, "sub-preserved");
    server.abort();
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[test]
fn stripe_meter_receipt_matches_the_legacy_javascript_contract() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/stripe-meter-delivery.json"
    ))
    .unwrap();
    let event: MeterEvent = serde_json::from_value(fixture["event"].clone()).unwrap();
    let (key, payload) = crate::billing::payments::stripe_meter_command(
        &event,
        "cus_original",
        fixture["nowMs"].as_i64().unwrap(),
    )
    .unwrap();
    assert_eq!(key, fixture["identifier"].as_str().unwrap());
    assert_eq!(payload, fixture["serializedPayload"].as_str().unwrap());
}
