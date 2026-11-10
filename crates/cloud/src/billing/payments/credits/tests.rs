use super::*;
use crate::{
    auth::{Identity, Membership, MembershipRole, Principal},
    routes::{AppState, router},
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Form, Path, Query, State},
    http::{HeaderMap, Request, StatusCode},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
};
use tower::ServiceExt;
struct Provider {
    balance: AtomicI64,
    owner: Mutex<String>,
    price_valid: AtomicBool,
    forms: Mutex<Vec<HashMap<String, String>>>,
    purchases: Mutex<HashMap<String, Value>>,
    refunds: Mutex<Vec<Value>>,
    adjustments: Mutex<HashMap<String, i64>>,
    adjustment_calls: AtomicUsize,
    fail_adjustment: AtomicBool,
}
fn auth(h: &HeaderMap) {
    assert_eq!(h["authorization"], "Bearer fixture-key");
}
async fn customer(State(f): State<Arc<Provider>>, h: HeaderMap) -> Json<Value> {
    auth(&h);
    Json(
        json!({"id":"cus_original","metadata":{"account_id":f.owner.lock().unwrap().clone()},"currency":"usd","balance":f.balance.load(Ordering::SeqCst)}),
    )
}
async fn price(State(f): State<Arc<Provider>>, h: HeaderMap) -> Json<Value> {
    auth(&h);
    Json(
        json!({"id":"price_credit","active":true,"currency":"usd","unit_amount":if f.price_valid.load(Ordering::SeqCst){100}else{200},"recurring":null,"billing_scheme":"per_unit","transform_quantity":null}),
    )
}
async fn checkout(
    State(f): State<Arc<Provider>>,
    h: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    auth(&h);
    assert_eq!(form["mode"], "payment");
    assert_eq!(form["customer"], "cus_original");
    assert_eq!(form["client_reference_id"], "user-alice");
    assert_eq!(form["allowed_payment_method_types[0]"], "card");
    assert_eq!(form["metadata[purpose]"], PURPOSE);
    assert_eq!(form["payment_intent_data[metadata][purpose]"], PURPOSE);
    assert_eq!(form["line_items[0][price]"], "price_credit");
    assert!(!form.contains_key("allow_promotion_codes"));
    assert!(form["success_url"].ends_with("/v1/billing/prepaid/complete"));
    f.forms.lock().unwrap().push(form);
    Json(json!({"url":"https://stripe.example.test/credits/fixture"}))
}
async fn purchase(
    State(f): State<Arc<Provider>>,
    Path(id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
    h: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    auth(&h);
    assert!(query.contains(&("expand[]".into(), "payment_intent.latest_charge".into())));
    assert!(query.contains(&("expand[]".into(), "line_items".into())));
    f.purchases
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
async fn charge(Path(id): Path<String>, h: HeaderMap) -> Json<Value> {
    auth(&h);
    Json(json!({"id":id,"payment_intent":"pi_original"}))
}
async fn sessions(Query(query): Query<HashMap<String, String>>, h: HeaderMap) -> Json<Value> {
    auth(&h);
    assert_eq!(query["payment_intent"], "pi_original");
    assert_eq!(query["limit"], "2");
    Json(json!({"data":[{"id":"cs_prepaid"}],"has_more":false}))
}
async fn refunds(
    State(f): State<Arc<Provider>>,
    Query(query): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Json<Value> {
    auth(&h);
    assert_eq!(query["limit"], "100");
    let list = f.refunds.lock().unwrap();
    if list.len() > 1 && !query.contains_key("starting_after") {
        Json(json!({"data":[list[0]],"has_more":true}))
    } else if list.len() > 1 {
        assert_eq!(query["starting_after"], list[0]["id"].as_str().unwrap());
        Json(json!({"data":list[1..],"has_more":false}))
    } else {
        Json(json!({"data":*list,"has_more":false}))
    }
}
async fn adjustment(
    State(f): State<Arc<Provider>>,
    h: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    auth(&h);
    f.adjustment_calls.fetch_add(1, Ordering::SeqCst);
    let key = h["idempotency-key"].to_str().unwrap();
    assert_eq!(form["metadata[zuse_operation]"], key);
    assert_eq!(form["currency"], "usd");
    assert_eq!(form["description"], "Zuse prepaid invoice balance");
    let amount = form["amount"].parse::<i64>().unwrap();
    let mut adjustments = f.adjustments.lock().unwrap();
    if let Some(old) = adjustments.get(key) {
        assert_eq!(*old, amount);
    } else {
        adjustments.insert(key.into(), amount);
        f.balance.fetch_add(amount, Ordering::SeqCst);
    }
    if f.fail_adjustment.swap(false, Ordering::SeqCst) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(Json(json!({"id":"cbtxn_fixture"})))
}
fn canonical(id: &str, amount: i64) -> Value {
    json!({"id":id,"mode":"payment","client_reference_id":"user-alice","customer":"cus_original","metadata":{"purpose":PURPOSE,"account_id":"user-alice"},"amount_total":amount,"payment_status":"paid","currency":"usd","line_items":{"data":[{"price":{"id":"price_credit"},"quantity":amount/100}],"has_more":false},"payment_intent":{"id":"pi_original","status":"succeeded","amount_received":amount,"latest_charge":{"id":"ch_original","amount":amount,"currency":"usd","paid":true,"captured":true,"amount_captured":amount,"disputed":false}}})
}
struct Accounts;
#[async_trait]
impl Identity for Accounts {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if !["alice", "bob", "admin", "member", "billing", "dev-user"].contains(&token) {
            return Err(ApiError::unauthorized("invalid_workos_token"));
        }
        Ok(Principal {
            account_id: if token == "dev-user" {
                "user-alice".into()
            } else {
                format!("user-{token}")
            },
        })
    }
    async fn membership(&self, account: &str, org: &str) -> Result<Membership, ApiError> {
        let role = account.strip_prefix("user-").unwrap_or("");
        if org != "org-team" || !["admin", "member", "billing"].contains(&role) {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(Membership {
            id: role.into(),
            user_id: account.into(),
            organization_id: org.into(),
            status: "active".into(),
            role: MembershipRole { slug: role.into() },
        })
    }
}
async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    scope: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(scope) = scope {
        r = r.header("x-zuse-workspace", scope);
    }
    let response = app
        .clone()
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let data = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&data).unwrap())
}
async fn deliver(app: &Router, kind: &str, object: Value) -> (StatusCode, Value) {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let body = json!({"id":format!("evt-{kind}"),"type":kind,"data":{"object":object}}).to_string();
    let ts = chrono::Utc::now().timestamp();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_fixture").unwrap();
    mac.update(format!("{ts}.{body}").as_bytes());
    let sig = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/billing/webhook/stripe")
                .header("stripe-signature", format!("t={ts},v1={sig}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn prepaid_credit_funding_refunds_disputes_and_legacy_receipts() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("Explicit isolated PostgreSQL required");
    assert!(matches!(
        Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("billing_credits_{}", uuid::Uuid::new_v4().simple());
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
    sqlx::raw_sql("CREATE TABLE api_stripe_customers(account_id text PRIMARY KEY,customer_id text UNIQUE,created_at bigint NOT NULL,generation integer NOT NULL DEFAULT 0,reservation_created_at bigint,recovery_cursor text,recovery_matches jsonb NOT NULL DEFAULT '[]',recovery_complete boolean NOT NULL DEFAULT FALSE,recovery_attempted_at bigint,recovery_lease_until bigint NOT NULL DEFAULT 0); INSERT INTO api_stripe_customers(account_id,customer_id,created_at) VALUES('user-alice','cus_original',100); CREATE TABLE api_stripe_meter_deliveries(delivery_key text PRIMARY KEY,payload text NOT NULL,first_attempt_at bigint NOT NULL,lease_until bigint NOT NULL DEFAULT 0,sent_at bigint); INSERT INTO zuse_rust_control_owners VALUES('billing','user-alice','rust',1,0)").execute(&pool).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO api_cloud_billing_periods VALUES('p-alice','user-alice','stripe','sub-preserved','active','USD',$1,$2,40000000,35000000,500,31000000,$1)").bind(now-86400000).bind(now+86400000).execute(&pool).await.unwrap();
    let f = Arc::new(Provider {
        balance: AtomicI64::new(0),
        owner: Mutex::new("user-alice".into()),
        price_valid: AtomicBool::new(true),
        forms: Mutex::new(vec![]),
        purchases: Mutex::new(HashMap::from([
            ("cs_prepaid".into(), canonical("cs_prepaid", 2500)),
            ("cs_legacy".into(), canonical("cs_legacy", 1000)),
            ("cs_ui".into(), canonical("cs_ui", 5000)),
        ])),
        refunds: Mutex::new(vec![]),
        adjustments: Mutex::new(HashMap::new()),
        adjustment_calls: AtomicUsize::new(0),
        fail_adjustment: AtomicBool::new(false),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let provider_server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/customers/cus_original", get(customer))
                .route("/prices/price_credit", get(price))
                .route("/checkout/sessions", post(checkout).get(sessions))
                .route("/checkout/sessions/{id}", get(purchase))
                .route("/charges/{id}", get(charge))
                .route("/refunds", get(refunds))
                .route(
                    "/customers/cus_original/balance_transactions",
                    post(adjustment),
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
        meter_price: None,
        http: http(),
        return_url: "https://api-rust.zuse.sh".into(),
        configuration: None,
        credit_price: Some("price_credit".into()),
        events: Some(events::EventConfig::fixture("whsec_fixture", "price_cloud")),
        checkout: None,
    });
    let signing = ed25519_dalek::SigningKey::from_bytes(&[20; 32]);
    let signer=crate::tickets::TicketSigner::from_jwk(&json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([20;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())}).to_string(),"https://api-rust.zuse.sh").unwrap();
    let payments = PaymentProviders::new("stripe".into(), vec![stripe])
        .unwrap()
        .with_checkout(signer);
    let app = router(
        AppState {
            store: Store::from_pool(pool.clone()).with_billing_control(
                BillingControl::from_pool(pool.clone(), vec!["user-alice".into()])
                    .with_payment_providers(payments),
            ),
            identity: Arc::new(Accounts),
            organizations_enabled: true,
            organization_ids: Arc::new(vec!["org-team".into()]),
            runtime: None,
            gateway: Arc::new(crate::gateway::Hub::default()),
        },
        vec![],
    );
    let balance = "/v1/billing/prepaid";
    let buy = "/v1/billing/prepaid/checkout";
    assert_eq!(
        request(&app, "GET", balance, "", None, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "GET",
            balance,
            "member",
            Some("organization:org-team"),
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", balance, "alice", None, json!({}))
            .await
            .1["creditCents"],
        0
    );
    assert_eq!(
        request(&app, "POST", buy, "alice", None, json!({"amountCents":999}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    f.price_valid.store(false, Ordering::SeqCst);
    assert_eq!(
        request(
            &app,
            "POST",
            buy,
            "alice",
            None,
            json!({"amountCents":2500})
        )
        .await
        .1["error"],
        "billing_credit_reconciliation_required"
    );
    assert!(f.forms.lock().unwrap().is_empty());
    f.price_valid.store(true, Ordering::SeqCst);
    assert_eq!(
        request(
            &app,
            "POST",
            buy,
            "alice",
            None,
            json!({"amountCents":2500})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.forms.lock().unwrap()[0]["line_items[0][quantity]"], "25");
    f.purchases.lock().unwrap().get_mut("cs_prepaid").unwrap()["payment_status"] = json!("unpaid");
    assert_eq!(
        deliver(
            &app,
            "checkout.session.completed",
            json!({"id":"cs_prepaid"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.balance.load(Ordering::SeqCst), 0);
    f.purchases
        .lock()
        .unwrap()
        .insert("cs_prepaid".into(), canonical("cs_prepaid", 2500));
    f.purchases.lock().unwrap().get_mut("cs_prepaid").unwrap()["line_items"]["data"][0]["price"]
        ["id"] = json!("wrong_price");
    assert_eq!(
        deliver(
            &app,
            "checkout.session.completed",
            json!({"id":"cs_prepaid"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.balance.load(Ordering::SeqCst), 0);
    f.purchases
        .lock()
        .unwrap()
        .insert("cs_prepaid".into(), canonical("cs_prepaid", 2500));
    f.fail_adjustment.store(true, Ordering::SeqCst);
    assert_eq!(
        deliver(
            &app,
            "checkout.session.completed",
            json!({"id":"cs_prepaid"})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(f.balance.load(Ordering::SeqCst), -2500);
    assert_eq!(
        deliver(
            &app,
            "checkout.session.completed",
            json!({"id":"cs_prepaid"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.balance.load(Ordering::SeqCst), -2500);
    assert_eq!(f.adjustments.lock().unwrap().len(), 1);
    let calls = f.adjustment_calls.load(Ordering::SeqCst);
    let (a, b) = tokio::join!(
        deliver(
            &app,
            "checkout.session.completed",
            json!({"id":"cs_prepaid"})
        ),
        deliver(
            &app,
            "checkout.session.async_payment_succeeded",
            json!({"id":"cs_prepaid"})
        )
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(f.adjustment_calls.load(Ordering::SeqCst), calls);
    // An original-format historical receipt remains authoritative after the retry window.
    sqlx::query(
        "INSERT INTO api_stripe_meter_deliveries VALUES('prepaid:purchase:cs_legacy',$1,$2,0,$2)",
    )
    .bind(r#"{"customerId":"cus_original","amount":-1000,"purchaseId":"cs_legacy"}"#)
    .bind(now - 30 * 3600000)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        deliver(
            &app,
            "checkout.session.completed",
            json!({"id":"cs_legacy"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.adjustment_calls.load(Ordering::SeqCst), calls);
    f.purchases.lock().unwrap().get_mut("cs_prepaid").unwrap()["payment_intent"]["latest_charge"]
        ["disputed"] = json!(true);
    assert_eq!(
        deliver(
            &app,
            "charge.dispute.funds_reinstated",
            json!({"id":"dp_original","charge":"ch_original","amount":1000})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.balance.load(Ordering::SeqCst), -2500);
    assert_eq!(
        deliver(
            &app,
            "charge.dispute.funds_withdrawn",
            json!({"id":"dp_original","charge":"ch_original","amount":1000})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.balance.load(Ordering::SeqCst), -2500);
    f.purchases.lock().unwrap().get_mut("cs_prepaid").unwrap()["payment_intent"]["latest_charge"]
        ["disputed"] = json!(false);
    *f.refunds.lock().unwrap() = vec![
        json!({"id":"re_one","amount":500,"status":"succeeded"}),
        json!({"id":"re_two","amount":500,"status":"succeeded"}),
    ];
    assert_eq!(
        deliver(&app, "charge.refunded", json!({"id":"ch_original"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(f.balance.load(Ordering::SeqCst), -1500);
    let calls = f.adjustment_calls.load(Ordering::SeqCst);
    assert_eq!(
        deliver(&app, "refund.updated", json!({"charge":"ch_original"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(f.adjustment_calls.load(Ordering::SeqCst), calls);
    f.refunds
        .lock()
        .unwrap()
        .push(json!({"id":"re_aged","amount":500,"status":"succeeded"}));
    sqlx::query(
        "INSERT INTO api_stripe_meter_deliveries VALUES('prepaid:refund:re_aged',$1,$2,0,NULL)",
    )
    .bind(r#"{"customerId":"cus_original","amount":500,"purchaseId":"cs_prepaid"}"#)
    .bind(now - 24 * 3600000)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        deliver(&app, "charge.refunded", json!({"id":"ch_original"}))
            .await
            .1["error"],
        "billing_credit_reconciliation_required"
    );
    assert_eq!(f.adjustment_calls.load(Ordering::SeqCst), calls);
    sqlx::query(
        "UPDATE zuse_rust_control_owners SET backend='legacy' WHERE resource_id='user-alice'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        deliver(
            &app,
            "checkout.session.completed",
            json!({"id":"cs_prepaid"})
        )
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
    *f.owner.lock().unwrap() = "foreign".into();
    assert_eq!(
        request(&app, "GET", balance, "alice", None, json!({}))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    *f.owner.lock().unwrap() = "user-alice".into();
    assert_eq!(
        request(&app, "GET", balance, "alice", None, json!({}))
            .await
            .1["creditCents"],
        1500
    );
    let policy:(i64,i64)=sqlx::query_as("SELECT included_provider_cost_micros,overage_cap_micros FROM api_cloud_billing_periods WHERE period_id='p-alice'").fetch_one(&pool).await.unwrap();
    assert_eq!(policy, (35000000, 31000000));
    f.refunds.lock().unwrap().clear();
    f.forms.lock().unwrap().clear();
    if let Ok(path) = std::env::var("ZUSE_CLOUD_CREDITS_UI_FIXTURE_FILE") {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let api = tokio::spawn(axum::serve(listener, app.clone()).into_future());
        let file = std::path::Path::new(&path);
        std::fs::write(file, origin).unwrap();
        tokio::time::timeout(Duration::from_secs(900), async {
            let mut funded = false;
            while !file.with_extension("done").exists() {
                if !funded && file.with_extension("fund").exists() {
                    assert_eq!(
                        deliver(&app, "checkout.session.completed", json!({"id":"cs_ui"}))
                            .await
                            .0,
                        StatusCode::OK
                    );
                    funded = true;
                    std::fs::write(file.with_extension("funded"), "verified").unwrap();
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(funded);
        })
        .await
        .expect("Prepaid UI acceptance timed out");
        assert_eq!(f.balance.load(Ordering::SeqCst), -6500);
        let forms = f.forms.lock().unwrap();
        assert_eq!(forms.len(), 2);
        assert!(forms.iter().any(|f| f["line_items[0][quantity]"] == "25"));
        assert!(forms.iter().any(|f| f["line_items[0][quantity]"] == "50"));
        api.abort();
    }
    provider_server.abort();
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
