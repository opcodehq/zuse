use super::*;
use crate::{
    auth::{Identity, Membership, Principal},
    routes::{AppState, router},
};
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{Request, StatusCode},
    routing::get,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

fn config(secret: &str, price: &str) -> EventConfig {
    EventConfig {
        secret: Zeroizing::new(secret.into()),
        offers: HashMap::from([
            (price.into(), "cloud-workspace-standard-v1".into()),
            ("price_machine".into(), "persistent-standard-v1".into()),
        ]),
    }
}
fn sign(secret: &str, prefix: &str, body: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(prefix.as_bytes());
    mac.update(body.as_bytes());
    mac.finalize().into_bytes().to_vec()
}
fn stripe_headers(body: &str, now: i64) -> HeaderMap {
    let timestamp = (now / 1000).to_string();
    let sig = sign("whsec_fixture", &format!("{timestamp}."), body);
    let encoded = sig.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut h = HeaderMap::new();
    h.insert(
        "stripe-signature",
        format!("t={timestamp},v0=ignored,v1=bad,v1={encoded}")
            .parse()
            .unwrap(),
    );
    h
}
fn polar_headers(body: &str, now: i64) -> HeaderMap {
    let timestamp = (now / 1000).to_string();
    let id = "polar-delivery-original";
    let sig = sign("polar_whs_fixture", &format!("{id}.{timestamp}."), body);
    let mut h = HeaderMap::new();
    h.insert("webhook-id", id.parse().unwrap());
    h.insert("webhook-timestamp", timestamp.parse().unwrap());
    h.insert(
        "webhook-signature",
        format!("v1,{}", STANDARD.encode(sig)).parse().unwrap(),
    );
    h
}
#[test]
fn signatures_bind_exact_bytes_timestamps_and_polar_delivery_id() {
    let now = 1_800_000_000_000;
    let body = r#"{"id":"evt_original","type":"customer.subscription.updated","data":{"object":{"id":"sub_original"}}}"#;
    let stripe = config("whsec_fixture", "price_cloud");
    let headers = stripe_headers(body, now);
    assert_eq!(
        stripe
            .stripe_event(&headers, body.as_bytes(), now)
            .unwrap()
            .unwrap()
            .subscription_id,
        "sub_original"
    );
    assert!(
        stripe
            .stripe_event(&headers, format!("{body} ").as_bytes(), now)
            .is_err()
    );
    assert!(
        stripe
            .stripe_event(&headers, body.as_bytes(), now + 301000)
            .is_err()
    );
    assert!(
        stripe
            .stripe_event(&headers, body.as_bytes(), now - 301000)
            .is_err()
    );
    let body = r#"{"type":"subscription.updated","data":{"id":"polar_original"}}"#;
    let polar = config("polar_whs_fixture", "product_cloud");
    let mut headers = polar_headers(body, now);
    assert_eq!(
        polar
            .polar_event(&headers, body.as_bytes(), now)
            .unwrap()
            .unwrap()
            .event_id,
        "polar-delivery-original"
    );
    let standard = config(
        &format!("whsec_{}", STANDARD.encode("new-fixture-key")),
        "product_cloud",
    );
    let sig = sign(
        "new-fixture-key",
        &format!("polar-delivery-original.{}.", now / 1000),
        body,
    );
    let mut standard_headers = headers.clone();
    standard_headers.insert(
        "webhook-signature",
        format!("v1,{}", STANDARD.encode(sig)).parse().unwrap(),
    );
    assert!(
        standard
            .polar_event(&standard_headers, body.as_bytes(), now)
            .unwrap()
            .is_some()
    );
    headers.insert("webhook-id", "foreign-delivery".parse().unwrap());
    assert!(polar.polar_event(&headers, body.as_bytes(), now).is_err());
}
struct Fixture {
    subscriptions: Mutex<HashMap<String, Value>>,
    reads: AtomicUsize,
}
async fn subscription(
    State(f): State<Arc<Fixture>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    assert_eq!(headers["authorization"], "Bearer fixture-key");
    f.reads.fetch_add(1, Ordering::SeqCst);
    f.subscriptions
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
struct AccountIdentity;
#[async_trait]
impl Identity for AccountIdentity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if token != "alice" {
            return Err(ApiError::unauthorized("invalid_workos_token"));
        }
        Ok(Principal {
            account_id: "user-alice".into(),
        })
    }
    async fn membership(&self, _: &str, _: &str) -> Result<Membership, ApiError> {
        Err(ApiError::forbidden("organization_access_denied"))
    }
}
fn stripe_subscription(id: &str, status: &str, start: i64, end: i64) -> Value {
    json!({"id":id,"status":status,"customer":{"id":"cus_original","metadata":{"account_id":"user-alice"}},"metadata":{"account_id":"user-alice","offer_id":"cloud-workspace-standard-v1"},"items":{"data":[{"price":{"id":"price_cloud"},"current_period_start":start/1000,"current_period_end":end/1000}]}})
}
async fn deliver(app: &Router, provider: &str, body: Value, now: i64) -> (StatusCode, Value) {
    let body = body.to_string();
    let h = if provider == "stripe" {
        stripe_headers(&body, now)
    } else {
        polar_headers(&body, now)
    };
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/v1/billing/webhook/{provider}"));
    *request.headers_mut().unwrap() = h;
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
fn event(id: &str, subscription: &str) -> Value {
    json!({"id":id,"type":"customer.subscription.updated","data":{"object":{"id":subscription,"status":"canceled","metadata":{"account_id":"untrusted-payload-owner"}}}})
}
async fn query_text(pool: &PgPool, sql: &str) -> String {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn signed_provider_delivery_preserves_subscriptions_periods_and_money() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("Explicit isolated PostgreSQL required");
    assert!(matches!(
        Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("billing_events_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
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
    sqlx::raw_sql("ALTER TABLE api_entitlements ADD COLUMN period_start bigint; ALTER TABLE api_entitlements ADD COLUMN ended_at bigint; ALTER TABLE api_cloud_billing_periods ADD COLUMN price_catalog_version text; ALTER TABLE api_cloud_billing_periods ADD COLUMN created_at bigint; ALTER TABLE api_cloud_billing_periods ADD UNIQUE(account_id,period_start); CREATE TABLE api_billing_events(provider text NOT NULL,event_id text NOT NULL,payload text,received_at bigint NOT NULL,processed_at bigint,PRIMARY KEY(provider,event_id)); CREATE TABLE api_stripe_customers(account_id text PRIMARY KEY,customer_id text UNIQUE); INSERT INTO api_stripe_customers VALUES('user-alice','cus_original'); INSERT INTO zuse_rust_control_owners VALUES('billing','user-alice','rust',1,0),('billing','user-bob','rust',1,0)").execute(&pool).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis() / 1000 * 1000;
    let start = now - 86400000;
    let end = now + 86400000;
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,billing_provider,provider_subscription_id,status,currency,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,overage_cap_micros,updated_at,created_at,price_catalog_version) VALUES('legacy-period','user-alice','stripe','sub-preserved','active','USD',$1,$2,42000000,35000000,750,31000000,100,100,'legacy-price-version')").bind(start).bind(end).execute(&pool).await.unwrap();
    sqlx::raw_sql("INSERT INTO api_cloud_billing_ledger VALUES('old-cost','legacy-period','provider-cost',70000000),('old-charge','legacy-period','overage-charge',10000000)").execute(&pool).await.unwrap();
    let fixture = Arc::new(Fixture {
        subscriptions: Mutex::new(HashMap::from([(
            "sub-preserved".into(),
            stripe_subscription("sub-preserved", "active", start, end),
        )])),
        reads: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/subscriptions/{id}", get(subscription))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let http = || ProviderHttp {
        client: Client::new(),
        origin: Url::parse(&origin).unwrap(),
        secret: Zeroizing::new("fixture-key".into()),
    };
    let providers = PaymentProviders::new(
        "stripe".into(),
        vec![
            Arc::new(StripePortal {
                http: http(),
                events: Some(config("whsec_fixture", "price_cloud")),
                return_url: "https://api-rust.zuse.sh".into(),
                configuration: None,
            }),
            Arc::new(PolarPortal {
                http: http(),
                events: Some(config("polar_whs_fixture", "product_cloud")),
            }),
        ],
    )
    .unwrap();
    let store = Store::from_pool(pool.clone()).with_billing_control(
        BillingControl::from_pool(pool.clone(), vec!["user-alice".into(), "user-bob".into()])
            .with_payment_providers(providers),
    );
    let app = router(
        AppState {
            store,
            identity: Arc::new(AccountIdentity),
            organizations_enabled: false,
            organization_ids: Arc::new(vec![]),
            runtime: None,
            gateway: Arc::new(crate::gateway::Hub::default()),
        },
        vec![],
    );
    assert_eq!(
        deliver(
            &app,
            "stripe",
            event("stale", "sub-preserved"),
            now - 301000
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        deliver(&app, "stripe", event("active", "sub-preserved"), now)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        query_text(
            &pool,
            "SELECT status FROM api_entitlements WHERE entitlement_id='ent-alice'"
        )
        .await,
        "active"
    );
    assert_eq!(
        deliver(&app, "stripe", event("active", "sub-preserved"), now)
            .await
            .1["duplicate"],
        true
    );
    let (a, b) = tokio::join!(
        deliver(&app, "stripe", event("concurrent", "sub-preserved"), now),
        deliver(&app, "stripe", event("concurrent", "sub-preserved"), now)
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_ne!(a.1["duplicate"], b.1["duplicate"]);
    fixture.subscriptions.lock().unwrap().insert(
        "sub-preserved".into(),
        stripe_subscription("sub-preserved", "past_due", start, end),
    );
    let failed = json!({"id":"payment-failed","type":"invoice.payment_failed","data":{"object":{"parent":{"subscription_details":{"subscription":"sub-preserved"}}}}});
    assert_eq!(deliver(&app, "stripe", failed, now).await.0, StatusCode::OK);
    assert_eq!(
        query_text(
            &pool,
            "SELECT status FROM api_cloud_billing_periods WHERE period_id='legacy-period'"
        )
        .await,
        "billing-hold"
    );
    fixture.subscriptions.lock().unwrap().insert(
        "sub-preserved".into(),
        stripe_subscription("sub-preserved", "active", start, end),
    );
    assert_eq!(
        deliver(
            &app,
            "stripe",
            event("old-notification", "sub-preserved"),
            now
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        query_text(
            &pool,
            "SELECT status FROM api_cloud_billing_periods WHERE period_id='legacy-period'"
        )
        .await,
        "active"
    );
    fixture.subscriptions.lock().unwrap().insert(
        "sub-preserved".into(),
        stripe_subscription("sub-preserved", "canceled", start, end),
    );
    assert_eq!(
        deliver(&app, "stripe", event("cancel-future", "sub-preserved"), now)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        query_text(
            &pool,
            "SELECT status FROM api_cloud_billing_periods WHERE period_id='legacy-period'"
        )
        .await,
        "active"
    );
    fixture.subscriptions.lock().unwrap().insert(
        "sub-preserved".into(),
        stripe_subscription("sub-preserved", "canceled", start, now - 1000),
    );
    assert_eq!(
        deliver(
            &app,
            "stripe",
            event("cancel-expired", "sub-preserved"),
            now
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        query_text(
            &pool,
            "SELECT status FROM api_cloud_billing_periods WHERE period_id='legacy-period'"
        )
        .await,
        "ended"
    );
    fixture.subscriptions.lock().unwrap().insert(
        "sub-preserved".into(),
        stripe_subscription("sub-preserved", "active", end, end + 30 * 86400000),
    );
    assert_eq!(
        deliver(&app, "stripe", event("renew", "sub-preserved"), now)
            .await
            .0,
        StatusCode::OK
    );
    let cap:i64=sqlx::query_scalar("SELECT overage_cap_micros FROM api_cloud_billing_periods WHERE period_start=$1 AND account_id='user-alice'").bind(end).fetch_one(&pool).await.unwrap();
    assert_eq!(cap, 31000000);
    // A competing subscription cannot replace the original period owner. Its receipt rolls back too.
    fixture.subscriptions.lock().unwrap().insert(
        "sub-competing".into(),
        stripe_subscription("sub-competing", "active", end, end + 30 * 86400000),
    );
    assert_eq!(
        deliver(&app, "stripe", event("competing", "sub-competing"), now)
            .await
            .1["error"],
        "billing_period_owner_conflict"
    );
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_billing_events WHERE event_id='competing'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(receipts, 0);
    let new_ent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_entitlements WHERE provider_subscription_id='sub-competing'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(new_ent, 0);
    // Lost ownership cannot consume a delivery or rewrite the retained subscription.
    sqlx::query(
        "UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_id='user-alice'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        deliver(&app, "stripe", event("fenced", "sub-preserved"), now)
            .await
            .1["error"],
        "cloud_billing_owner_not_rust"
    );
    sqlx::query(
        "UPDATE zuse_rust_control_owners SET backend='rust' WHERE resource_id='user-alice'",
    )
    .execute(&pool)
    .await
    .unwrap();
    fixture
        .subscriptions
        .lock()
        .unwrap()
        .get_mut("sub-preserved")
        .unwrap()["metadata"]["account_id"] = json!("user-bob");
    assert_eq!(
        deliver(&app, "stripe", event("wrong-owner", "sub-preserved"), now)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    fixture
        .subscriptions
        .lock()
        .unwrap()
        .get_mut("sub-preserved")
        .unwrap()["metadata"]["account_id"] = json!("user-alice");
    sqlx::query(
        "UPDATE api_stripe_customers SET customer_id='cus_foreign' WHERE account_id='user-alice'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        deliver(
            &app,
            "stripe",
            event("wrong-customer", "sub-preserved"),
            now
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    sqlx::query("DELETE FROM api_stripe_customers WHERE account_id='user-alice'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        deliver(&app, "stripe", event("unlinked", "sub-preserved"), now)
            .await
            .1["deferred"],
        "subscription-unlinked"
    );
    let polar = json!({"id":"legacy-sub","status":"past_due","customer":{"external_id":"user-bob"},"product_id":"product_cloud","current_period_start":chrono::DateTime::from_timestamp_millis(start).unwrap().to_rfc3339(),"current_period_end":chrono::DateTime::from_timestamp_millis(end).unwrap().to_rfc3339(),"metadata":{}});
    fixture
        .subscriptions
        .lock()
        .unwrap()
        .insert("legacy-sub".into(), polar);
    assert_eq!(
        deliver(
            &app,
            "polar",
            json!({"type":"subscription.updated","data":{"id":"legacy-sub"}}),
            now
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        query_text(
            &pool,
            "SELECT status FROM api_entitlements WHERE entitlement_id='ent-bob'"
        )
        .await,
        "grace"
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM api_entitlements WHERE entitlement_id IN ('ent-alice','ent-bob') AND created_at=100").fetch_one(&pool).await.unwrap();
    assert_eq!(count, 2);
    let retained:(i64,i64,i64,String)=sqlx::query_as("SELECT base_price_micros,markup_basis_points,overage_cap_micros,price_catalog_version FROM api_cloud_billing_periods WHERE period_id='legacy-period'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        retained,
        (42000000, 750, 31000000, "legacy-price-version".into())
    );
    let money: i64 =
        sqlx::query_scalar("SELECT sum(amount_micros)::bigint FROM api_cloud_billing_ledger")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(money, 80000000);
    let rejected:i64=sqlx::query_scalar("SELECT count(*) FROM api_billing_events WHERE event_id IN ('stale','fenced','wrong-owner','wrong-customer','unlinked')").fetch_one(&pool).await.unwrap();
    assert_eq!(rejected, 0);
    server.abort();
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
