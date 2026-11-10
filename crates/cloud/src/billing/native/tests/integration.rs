use super::*;
use axum::{
    Json, Router,
    extract::{Form, Path, State},
    http::HeaderMap,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::postgres::PgPoolOptions;
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
struct Processor {
    sessions: Mutex<HashMap<String, Value>>,
    checkout_posts: AtomicUsize,
    subscription: Mutex<Value>,
    paid: AtomicBool,
}
async fn create_customer(
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(headers["authorization"], "Bearer sk_test_fixture");
    Json(
        json!({"id":"cus_native","livemode":false,"metadata":{"brokeboy_customer":form["metadata[brokeboy_customer]"],"brokeboy_environment":"test"}}),
    )
}
async fn customer() -> Json<Value> {
    Json(
        json!({"id":"cus_native","livemode":false,"metadata":{"brokeboy_customer":"user-native","brokeboy_environment":"test"}}),
    )
}
async fn checkout(
    State(p): State<Arc<Processor>>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(form["customer"], "cus_native");
    let id = if form["mode"] == "payment" {
        "cs_credit"
    } else {
        "cs_cloud"
    };
    let mut metadata = serde_json::Map::new();
    for (key, value) in &form {
        if let Some(name) = key
            .strip_prefix("metadata[")
            .and_then(|k| k.strip_suffix(']'))
        {
            metadata.insert(name.into(), json!(value));
        }
    }
    let amount = form
        .get("line_items[0][price_data][unit_amount]")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(4000);
    let value = json!({"id":id,"livemode":false,"mode":form["mode"],"customer":"cus_native","currency":"usd","amount_total":amount,"payment_status":"paid","subscription":if id=="cs_cloud"{json!("sub_native")}else{Value::Null},"metadata":metadata,"payment_intent":{"status":"succeeded","amount_received":amount,"latest_charge":{"paid":true,"captured":true,"amount_captured":amount,"currency":"usd"}}});
    p.sessions.lock().unwrap().insert(id.into(), value);
    p.checkout_posts.fetch_add(1, Ordering::SeqCst);
    Json(json!({"id":id,"url":format!("https://checkout.stripe.com/{id}")}))
}
async fn session(State(p): State<Arc<Processor>>, Path(id): Path<String>) -> Json<Value> {
    let mut value = p.sessions.lock().unwrap()[&id].clone();
    if !p.paid.load(Ordering::SeqCst) {
        value["payment_status"] = json!("unpaid");
    }
    Json(value)
}
async fn subscription(State(p): State<Arc<Processor>>) -> Json<Value> {
    Json(p.subscription.lock().unwrap().clone())
}
async fn price() -> Json<Value> {
    Json(
        json!({"id":"price_native","livemode":false,"active":true,"currency":"usd","unit_amount":4000,"recurring":{"interval":"month"}}),
    )
}
async fn portal() -> Json<Value> {
    Json(json!({"url":"https://billing.stripe.com/native"}))
}
pub(super) fn signed(event: &Value) -> (HeaderMap, Vec<u8>) {
    let bytes = serde_json::to_vec(event).unwrap();
    let ts = chrono::Utc::now().timestamp();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_fixture").unwrap();
    mac.update(format!("{ts}.").as_bytes());
    mac.update(&bytes);
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut h = HeaderMap::new();
    h.insert(
        "stripe-signature",
        format!("t={ts},v1={signature}").parse().unwrap(),
    );
    (h, bytes)
}
pub(super) fn signer() -> crate::tickets::TicketSigner {
    let signing = ed25519_dalek::SigningKey::from_bytes(&[19; 32]);
    crate::tickets::TicketSigner::from_jwk(&json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([19;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())}).to_string(),"https://api.example.test").unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn native_commerce_webhooks_wallet_concurrency_and_recovery() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("isolated localhost PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("native_billing_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut scoped_url = reqwest::Url::parse(&url).unwrap();
    scoped_url
        .query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(scoped_url.as_str())
        .await
        .unwrap();
    for sql in [
        include_str!("../../../../migrations/0000_application.sql"),
        include_str!("../../../../migrations/0001_control_plane.sql"),
        MIGRATION,
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql(
        "INSERT INTO zuse_rust_control_owners VALUES('billing','user-native','rust',1,0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let processor = Arc::new(Processor::default());
    processor.paid.store(true, Ordering::SeqCst);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/customers", post(create_customer))
                .route("/customers/cus_native", get(customer))
                .route("/prices/price_native", get(price))
                .route("/checkout/sessions", post(checkout))
                .route("/checkout/sessions/{id}", get(session))
                .route("/subscriptions/sub_native", get(subscription))
                .route("/billing_portal/sessions", post(portal))
                .with_state(processor.clone()),
        )
        .into_future(),
    );
    let mut options =
        brokeboy::stripe::StripeOptions::new("sk_test_fixture".into(), "whsec_fixture".into());
    options.api_base = base;
    let adapter: Arc<dyn PaymentProvider> =
        Arc::new(brokeboy::stripe::Stripe::new(options).unwrap());
    let provider: Arc<dyn PaymentProvider> =
        Arc::new(Providers::new("stripe".into(), vec![adapter]).unwrap());
    let engine = Billing::open(
        DatabaseConnection {
            driver: Dialect::Postgres,
            url: scoped_url.to_string(),
            max_connections: Some(4),
            acquire_timeout_ms: Some(3000),
        },
        BillingOptions {
            provider: Some(provider.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let native = NativeBilling {
        engine: engine.clone(),
        provider,
        accounts: HashSet::from(["user-native".into()]),
        default_provider: "stripe".into(),
        return_origin: "https://api.example.test".into(),
        automatic_tax: false,
        signer: Some(signer()),
        stripe_cloud_price: Some("price_native".into()),
    };
    let control = BillingControl::from_pool(pool.clone(), vec!["user-native".into()])
        .with_native_billing(native);
    control.publish_native_catalog(json!({"features":[{"id":CREDIT_WALLET,"kind":"consumable"}],"plans":[{"id":CLOUD_PLAN,"name":"Zuse Cloud","kind":"subscription","currency":"usd","amount":"40","interval":"month","stripePriceId":"price_native","items":[{"feature":CREDIT_WALLET,"included":"40","reset":"month"}]}]}),0).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    control
        .enroll_native("user-native", false, now)
        .await
        .unwrap();
    assert!(control.native_for("other-user").await.unwrap().is_none());
    let first = control
        .native_checkout("user-native", CLOUD_PLAN, now, Some("request-1"))
        .await
        .unwrap();
    let replay = control
        .native_checkout("user-native", CLOUD_PLAN, now + 5000, Some("request-1"))
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(processor.checkout_posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        control.native_portal("user-native").await.unwrap().unwrap(),
        "https://billing.stripe.com/native"
    );
    assert_eq!(
        control
            .native_balance("user-native")
            .await
            .unwrap()
            .unwrap()
            .funding,
        "shared-credits"
    );
    assert!(
        control
            .native_receipt("other-user", "payment:request-1")
            .await
            .is_none()
    );

    // A real signed notification is accepted durably, then canonical provider state
    // provisions the host's access/period projection through the maintenance worker.
    let start = (now / 1000) - 60;
    let end = start + 2_592_000;
    *processor.subscription.lock().unwrap() = json!({"id":"sub_native","customer":"cus_native","livemode":false,"status":"active","latest_invoice":{"status":"paid"},"metadata":{"brokeboy_customer":"user-native","brokeboy_environment":"test","brokeboy_plan":CLOUD_PLAN},"cancel_at_period_end":false,"items":{"data":[{"id":"si_native","quantity":1,"current_period_start":start,"current_period_end":end,"price":{"id":"price_native","recurring":{"usage_type":"licensed"},"metadata":{"brokeboy_plan":CLOUD_PLAN}}}]}});
    let event = json!({"id":"evt_native_subscription","type":"customer.subscription.updated","livemode":false,"data":{"object":{"id":"sub_native","customer":"cus_native","metadata":{"brokeboy_customer":"user-native","brokeboy_environment":"test"}}}});
    let (headers, bytes) = signed(&event);
    assert!(
        control
            .native_webhook(Some("polar"), &headers, &bytes)
            .await
            .is_err()
    );
    assert!(
        control
            .native_webhook(Some("stripe"), &HeaderMap::new(), &bytes)
            .await
            .is_err()
    );
    control
        .native_webhook(Some("stripe"), &headers, &bytes)
        .await
        .unwrap()
        .unwrap();
    control
        .native_webhook(Some("stripe"), &headers, &bytes)
        .await
        .unwrap()
        .unwrap();
    control.native_maintenance().await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM api_entitlements WHERE account_id='user-native' AND status='active'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let cap = zuse_protocol::cloud_billing::BillingCapRequest {
        overage_cap_micros: 30_000_000,
        idempotency_key: "raise-cap".into(),
    };
    let store = crate::store::Store::from_pool(pool.clone()).with_billing_control(control.clone());
    assert_eq!(
        store
            .billing_set_cap("user-native", cap, now)
            .await
            .unwrap()
            .overage_cap_micros,
        30_000_000
    );
    // Reprojection must preserve a cap edit, even when its state hash repeats.
    control
        .sync_native_subscriptions("user-native")
        .await
        .unwrap();
    assert_eq!(
        store
            .billing_summary("user-native", now)
            .await
            .unwrap()
            .overage_cap_micros,
        30_000_000
    );

    control
        .native_prepaid_checkout("user-native", 1000, Some("topup-1"))
        .await
        .unwrap();
    control
        .native_prepaid_checkout("user-native", 1000, Some("topup-1"))
        .await
        .unwrap();
    assert!(
        control
            .native_prepaid_checkout("user-native", 2500, Some("topup-1"))
            .await
            .is_err()
    );
    let prepaid = processor.sessions.lock().unwrap()["cs_credit"].clone();
    let event = json!({"id":"evt_native_prepaid","type":"checkout.session.completed","livemode":false,"data":{"object":prepaid}});
    let (h, b) = signed(&event);
    processor.paid.store(false, Ordering::SeqCst);
    control
        .native_webhook(Some("stripe"), &h, &b)
        .await
        .unwrap();
    control.native_maintenance().await.unwrap();
    assert_eq!(
        control
            .native_balance("user-native")
            .await
            .unwrap()
            .unwrap()
            .credit_cents,
        4000
    );
    processor.paid.store(true, Ordering::SeqCst);
    let mut event = event.clone();
    event["id"] = json!("evt_native_prepaid_confirmed");
    event["type"] = json!("checkout.session.async_payment_succeeded");
    let (h, b) = signed(&event);
    control
        .native_webhook(Some("stripe"), &h, &b)
        .await
        .unwrap();
    control.native_maintenance().await.unwrap();
    control
        .native_webhook(Some("stripe"), &h, &b)
        .await
        .unwrap();
    control.native_maintenance().await.unwrap();
    assert_eq!(
        control
            .native_balance("user-native")
            .await
            .unwrap()
            .unwrap()
            .credit_cents,
        5000
    );
    let request = ModelReservation {
        request_id: "model-stream".into(),
        chat_id: "chat-one".into(),
        model: "hosted-model".into(),
        estimate_micros: 2_000_000,
    };
    let reservation = control
        .reserve_hosted_model("user-native", &request, now)
        .await
        .unwrap();
    assert_eq!(
        reservation,
        control
            .reserve_hosted_model("user-native", &request, now)
            .await
            .unwrap()
    );
    control
        .increase_hosted_model("user-native", &request.request_id, 3_000_000)
        .await
        .unwrap();
    control
        .settle_hosted_model("user-native", &request.request_id, 2_500_000)
        .await
        .unwrap();
    control
        .settle_hosted_model("user-native", &request.request_id, 2_500_000)
        .await
        .unwrap();
    assert!(
        control
            .settle_hosted_model("user-native", &request.request_id, 1_000_000)
            .await
            .is_err()
    );
    assert_eq!(
        control
            .native_balance("user-native")
            .await
            .unwrap()
            .unwrap()
            .credit_cents,
        4750
    );
    let mut tasks = vec![];
    for i in 0..12 {
        let c = control.clone();
        tasks.push(tokio::spawn(async move {
            c.reserve_hosted_model(
                "user-native",
                &ModelReservation {
                    request_id: format!("race-{i}"),
                    chat_id: "chat".into(),
                    model: "model".into(),
                    estimate_micros: 10_000_000,
                },
                now,
            )
            .await
            .is_ok()
        }));
    }
    let mut admitted = 0;
    for t in tasks {
        if t.await.unwrap() {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 4);
    assert_eq!(
        control
            .native_balance("user-native")
            .await
            .unwrap()
            .unwrap()
            .credit_cents,
        750
    );

    // Simulate reply loss after the engine debit committed but before the host ACK.
    sqlx::query("UPDATE zuse_brokeboy_model_requests SET state='settling' WHERE account_id='user-native' AND request_id='model-stream'").execute(&pool).await.unwrap();
    control.recover_model_requests("user-native").await.unwrap();
    let balance = control
        .native_balance("user-native")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(balance.credit_cents, 750);
    // Compute and models spend the same subscription-funded pool. A confirmed
    // $5 retail compute cost leaves $2.50 after the prior model reservations.
    let period: Value = sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id='user-native' ORDER BY period_start DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(period["shared_credit_account"], true);
    assert_eq!(period["included_provider_cost_micros"], 40_000_000);
    let mut period = period;
    period["markup_basis_points"] = json!(0);
    let resource = json!({"accountId":"user-native","kind":"workspace","id":"compute-one"});
    for _ in 0..2 {
        let mut tx = control.owned_transaction("user-native").await.unwrap();
        crate::billing::settlement::settle_segment(
            &mut tx,
            &resource,
            crate::billing::settlement::UsageIdentity {
                provider: "boxd",
                entry: "shared-compute",
                event: "shared-event",
                execution: "shared-execution",
                machine: None,
            },
            &period,
            now - 1000,
            now,
            5_000_000,
            1,
            1_048_576,
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(
        control
            .shared_available_micros("user-native")
            .await
            .unwrap(),
        2_500_000
    );
    control.settle_compute_credits("user-native").await.unwrap();
    // Lost host ACK replays the native debit, never charging twice.
    sqlx::query("UPDATE zuse_brokeboy_compute_charges SET state='pending'")
        .execute(&pool)
        .await
        .unwrap();
    control.settle_compute_credits("user-native").await.unwrap();
    assert_eq!(
        control
            .native_balance("user-native")
            .await
            .unwrap()
            .unwrap()
            .credit_cents,
        250
    );
    let invoices: i64 = sqlx::query_scalar("SELECT count(*) FROM api_cloud_billing_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(invoices, 0, "shared compute must not also be invoiced");
    let last = ModelReservation {
        request_id: "last-credits".into(),
        chat_id: "chat".into(),
        model: "model".into(),
        estimate_micros: 2_500_000,
    };
    control
        .reserve_hosted_model("user-native", &last, now)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        crate::billing::require_capacity(&mut tx, "user-native", now)
            .await
            .unwrap_err()
            .code,
        "shared_credit_exhausted"
    );
    tx.rollback().await.unwrap();
    control
        .settle_hosted_model("user-native", "last-credits", 2_500_000)
        .await
        .unwrap();
    assert_eq!(
        control
            .native_balance("user-native")
            .await
            .unwrap()
            .unwrap()
            .credit_cents,
        0
    );
    // A paid top-up replenishes the very same pool and clears the admission hold.
    engine.call("credits.grant",json!({"customerId":"user-native","feature":CREDIT_WALLET,"amount":"1","origin":"purchase","sourceId":"test-topup","idempotencyKey":"additional-topup"})).await.unwrap();
    control.sync_credit_capacity("user-native").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    crate::billing::require_capacity(&mut tx, "user-native", now)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let summary = store.billing_summary("user-native", now).await.unwrap();
    assert!(summary.shared_credits);
    assert_eq!(summary.included_remaining_micros, 1_000_000);
    assert_eq!(summary.current_invoice_estimate_micros, 40_000_000);
    // A late observation cannot silently debit a new month's allowance.
    sqlx::query(
        "UPDATE api_cloud_billing_periods SET period_end=$1 WHERE account_id='user-native'",
    )
    .bind(now - 1)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO zuse_brokeboy_compute_charges(account_id,entry_id,period_id,amount_micros,created_at) VALUES('user-native','late-compute',$1,500000,$2)").bind(period["period_id"].as_str().unwrap()).bind(now).execute(&pool).await.unwrap();
    control.settle_compute_credits("user-native").await.unwrap();
    let state: String = sqlx::query_scalar(
        "SELECT state FROM zuse_brokeboy_compute_charges WHERE entry_id='late-compute'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "reconciliation");
    let wallet = engine
        .call(
            "credits.balance",
            json!({"customerId":"user-native","feature":CREDIT_WALLET}),
        )
        .await
        .unwrap();
    assert_eq!(wallet["available"], "1");
    assert!(
        control
            .reserve_hosted_model(
                "user-native",
                &ModelReservation {
                    request_id: "late-blocked".into(),
                    chat_id: "chat".into(),
                    model: "model".into(),
                    estimate_micros: 1
                },
                now
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_kind='billing' AND resource_id='user-native'").execute(&pool).await.unwrap();
    assert!(control.native_portal("user-native").await.is_err());
    control.close_native_billing().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    pool.close().await;
    task.abort();
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
