use super::*;
use crate::{
    auth::{Identity, Membership, MembershipRole, Principal},
    routes::{AppState, router},
};
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Form, Path, Query, State},
    http::{HeaderMap, Request, StatusCode},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::postgres::PgPoolOptions;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;
struct Provider {
    customers: Mutex<HashMap<String, String>>,
    keys: Mutex<HashMap<String, String>>,
    forms: Mutex<Vec<HashMap<String, String>>>,
    creates: AtomicUsize,
    fail_create: AtomicBool,
    receipt_reads: AtomicUsize,
    list_mode: AtomicUsize,
}
fn authorized(headers: &HeaderMap) {
    assert_eq!(headers["authorization"], "Bearer fixture-key");
}
async fn customer(
    State(f): State<Arc<Provider>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    authorized(&h);
    let owner = f
        .customers
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(json!({"id":id,"metadata":{"account_id":owner}})))
}
async fn create_customer(
    State(f): State<Arc<Provider>>,
    h: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&h);
    f.creates.fetch_add(1, Ordering::SeqCst);
    let owner = &form["metadata[account_id]"];
    let key = h["idempotency-key"].to_str().unwrap();
    let id = f
        .keys
        .lock()
        .unwrap()
        .entry(key.into())
        .or_insert_with(|| format!("cus_{}", owner.replace(':', "_")))
        .clone();
    f.customers
        .lock()
        .unwrap()
        .insert(id.clone(), owner.clone());
    if f.fail_create.swap(false, Ordering::SeqCst) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(Json(json!({"id":id,"metadata":{"account_id":owner}})))
}
async fn customer_page(
    State(f): State<Arc<Provider>>,
    Query(query): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Json<Value> {
    authorized(&h);
    assert_eq!(query["limit"], "100");
    let page = match f.list_mode.load(Ordering::SeqCst) {
        1 if !query.contains_key("starting_after") => {
            json!({"data":[{"id":"cus_unrelated","metadata":{"account_id":"unrelated"}}],"has_more":true})
        }
        1 => {
            assert_eq!(query["starting_after"], "cus_unrelated");
            json!({"data":[{"id":"cus_recovered","metadata":{"account_id":"user-recovery"}}],"has_more":false})
        }
        2 => {
            json!({"data":[{"id":"cus_one","metadata":{"account_id":"user-ambiguous"}},{"id":"cus_two","metadata":{"account_id":"user-ambiguous"}}],"has_more":false})
        }
        _ => json!({"data":[],"has_more":false}),
    };
    Json(page)
}
async fn checkout(
    State(f): State<Arc<Provider>>,
    h: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    authorized(&h);
    assert_eq!(form["mode"], "subscription");
    assert_eq!(form["line_items[0][price]"], "price_cloud");
    assert_eq!(form["line_items[0][quantity]"], "1");
    assert_eq!(form["line_items[1][price]"], "price_meter");
    assert!(!form.contains_key("line_items[1][quantity]"));
    assert_eq!(form["metadata[account_id]"], form["client_reference_id"]);
    assert_eq!(form["subscription_data[metadata][offer_id]"], CLOUD_OFFER);
    assert_eq!(form["automatic_tax[enabled]"], "true");
    assert_eq!(form["billing_address_collection"], "required");
    assert!(form["success_url"].ends_with("checkout_id={CHECKOUT_SESSION_ID}"));
    let owner = f
        .customers
        .lock()
        .unwrap()
        .get(&form["customer"])
        .cloned()
        .unwrap();
    assert_eq!(owner, form["client_reference_id"]);
    f.forms.lock().unwrap().push(form);
    Json(json!({"url":"https://stripe.example.test/checkout/fixture"}))
}
async fn receipt(
    State(f): State<Arc<Provider>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Json<Value> {
    authorized(&h);
    f.receipt_reads.fetch_add(1, Ordering::SeqCst);
    Json(
        json!({"id":id,"client_reference_id":if id=="cs_original"{"user-alice"}else{"foreign"},"amount_total":4000,"currency":"usd","created":1800000000,"payment_status":"paid","status":"complete"}),
    )
}
async fn subscription(h: HeaderMap, Path(id): Path<String>) -> Json<Value> {
    authorized(&h);
    let now = chrono::Utc::now().timestamp();
    let (owner, customer) = if id == "sub-ui" {
        ("user-ui", "cus_user-ui")
    } else {
        ("user-alice", "cus_original")
    };
    Json(
        json!({"id":id,"status":"active","customer":{"id":customer,"metadata":{"account_id":owner}},"metadata":{"account_id":owner,"offer_id":CLOUD_OFFER},"items":{"data":[{"price":{"id":"price_cloud"},"current_period_start":now-10,"current_period_end":now+86400}]}}),
    )
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
                "user-ui".into()
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
fn signer() -> TicketSigner {
    let key = ed25519_dalek::SigningKey::from_bytes(&[19; 32]);
    TicketSigner::from_jwk(&json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([19;32]),"x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())}).to_string(),"https://api-rust.zuse.sh").unwrap()
}
async fn post_checkout(
    app: &Router,
    token: &str,
    scope: Option<&str>,
    offer: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1/billing/checkout")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(scope) = scope {
        request = request.header("x-zuse-workspace", scope);
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(json!({"offerId":offer}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn activate(store: &Store, id: &str) -> Value {
    let now = chrono::Utc::now().timestamp_millis();
    let body=json!({"id":format!("activation-{id}"),"type":"customer.subscription.created","data":{"object":{"id":id}}}).to_string();
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let timestamp = (now / 1000).to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec_fixture").unwrap();
    mac.update(format!("{timestamp}.{body}").as_bytes());
    let digest = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let mut headers = HeaderMap::new();
    headers.insert(
        "stripe-signature",
        format!("t={timestamp},v1={digest}").parse().unwrap(),
    );
    store
        .billing_webhook(Some("stripe"), &headers, body.as_bytes(), now)
        .await
        .unwrap()
}

#[test]
fn checkout_ticket_cannot_be_replaced_by_runtime_ticket_or_expired_proof() {
    let signer = signer();
    let now = 1_800_000_000_000;
    let ticket = signer.checkout("user-alice", CLOUD_OFFER, now).unwrap();
    let claims = signer.verify_checkout(&ticket, now).unwrap();
    assert_eq!(claims["sub"], "user-alice");
    assert_eq!(claims["offerId"], CLOUD_OFFER);
    assert!(signer.verify_checkout(&ticket, now + 86400000).is_err());
    assert!(
        signer
            .verify_checkout(
                &signer
                    .runtime("user-alice", "ws", 1, 1, now + 900000)
                    .unwrap(),
                now
            )
            .is_err()
    );
    assert!(signer.verify_checkout(&(ticket + "corrupt"), now).is_err());
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn checkout_reuses_identities_recovers_customers_and_activates_subscription() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("Explicit isolated PostgreSQL required");
    assert!(matches!(
        Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("billing_checkout_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
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
    sqlx::raw_sql("ALTER TABLE api_entitlements ADD COLUMN period_start bigint; ALTER TABLE api_entitlements ADD COLUMN ended_at bigint; UPDATE api_entitlements SET status='ended'; ALTER TABLE api_cloud_billing_periods ADD COLUMN price_catalog_version text; ALTER TABLE api_cloud_billing_periods ADD COLUMN created_at bigint; ALTER TABLE api_cloud_billing_periods ADD UNIQUE(account_id,period_start); CREATE TABLE api_billing_events(provider text,event_id text,payload text,received_at bigint,processed_at bigint,PRIMARY KEY(provider,event_id)); CREATE TABLE api_stripe_customers(account_id text PRIMARY KEY,customer_id text UNIQUE,created_at bigint NOT NULL,generation integer NOT NULL DEFAULT 0,reservation_created_at bigint,recovery_cursor text,recovery_matches jsonb NOT NULL DEFAULT '[]',recovery_complete boolean NOT NULL DEFAULT FALSE,recovery_attempted_at bigint,recovery_lease_until bigint NOT NULL DEFAULT 0); INSERT INTO api_stripe_customers(account_id,customer_id,created_at) VALUES('user-alice','cus_original',100); INSERT INTO zuse_rust_control_owners VALUES('billing','user-alice','rust',1,0),('billing','user-bob','rust',1,0),('billing','user-ui','rust',1,0),('billing','organization:org-team','rust',1,0)").execute(&pool).await.unwrap();
    let fixture = Arc::new(Provider {
        customers: Mutex::new(HashMap::from([
            ("cus_original".into(), "user-alice".into()),
            ("cus_recovered".into(), "user-recovery".into()),
        ])),
        keys: Mutex::new(HashMap::new()),
        forms: Mutex::new(vec![]),
        creates: AtomicUsize::new(0),
        fail_create: AtomicBool::new(false),
        receipt_reads: AtomicUsize::new(0),
        list_mode: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/customers", post(create_customer).get(customer_page))
                .route("/customers/{id}", get(customer))
                .route("/checkout/sessions", post(checkout))
                .route("/checkout/sessions/{id}", get(receipt))
                .route("/subscriptions/{id}", get(subscription))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let stripe = Arc::new(StripePortal {
        http: ProviderHttp {
            client: Client::new(),
            origin: Url::parse(&origin).unwrap(),
            secret: Zeroizing::new("fixture-key".into()),
        },
        return_url: "https://api-rust.zuse.sh".into(),
        configuration: None,
        events: Some(events::EventConfig::fixture("whsec_fixture", "price_cloud")),
        checkout: Some(ProviderCheckout {
            offers: HashMap::from([(CLOUD_OFFER.into(), "price_cloud".into())]),
            overage: Some("price_meter".into()),
            automatic_tax: true,
        }),
    });
    let providers = PaymentProviders::new("stripe".into(), vec![stripe.clone()])
        .unwrap()
        .with_checkout(signer());
    let store = Store::from_pool(pool.clone()).with_billing_control(
        BillingControl::from_pool(
            pool.clone(),
            vec![
                "user-alice".into(),
                "user-bob".into(),
                "user-ui".into(),
                "organization:org-team".into(),
            ],
        )
        .with_payment_providers(providers),
    );
    let state = AppState {
        store: store.clone(),
        identity: Arc::new(Accounts),
        organizations_enabled: true,
        organization_ids: Arc::new(vec!["org-team".into()]),
        runtime: None,
        gateway: Arc::new(crate::gateway::Hub::default()),
    };
    let app = router(state, vec![]);
    assert_eq!(
        post_checkout(&app, "", None, CLOUD_OFFER).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_checkout(&app, "member", Some("organization:org-team"), CLOUD_OFFER)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_checkout(
            &app,
            "admin",
            Some("organization:org-team"),
            "persistent-standard-v1"
        )
        .await
        .1["error"],
        "workspace_scope_not_supported"
    );
    assert_eq!(
        post_checkout(&app, "alice", None, "unknown").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_checkout(&app, "alice", None, CLOUD_OFFER).await.0,
        StatusCode::OK
    );
    assert_eq!(fixture.creates.load(Ordering::SeqCst), 0);
    let success = fixture.forms.lock().unwrap()[0]["success_url"].clone();
    let parsed = Url::parse(&success).unwrap();
    let ticket = parsed
        .query_pairs()
        .find(|(k, _)| k == "t")
        .unwrap()
        .1
        .into_owned();
    assert!(
        store
            .checkout_receipt(
                None,
                Some("cs_original"),
                chrono::Utc::now().timestamp_millis()
            )
            .await
            .is_none()
    );
    assert_eq!(fixture.receipt_reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .checkout_receipt(
                Some(&ticket),
                Some("cs_original"),
                chrono::Utc::now().timestamp_millis()
            )
            .await
            .unwrap()
            .amount_cents,
        4000
    );
    assert!(
        store
            .checkout_receipt(
                Some(&ticket),
                Some("cs_foreign"),
                chrono::Utc::now().timestamp_millis()
            )
            .await
            .is_none()
    );
    let path = format!("/v1/billing/checkout/complete?t={ticket}&checkout_id=cs_original");
    let response = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let html = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Payment confirmed") && html.contains("USD 40.00"));
    assert!(!html.contains("user-alice") && !html.contains("cus_original"));
    fixture.fail_create.store(true, Ordering::SeqCst);
    assert_eq!(
        post_checkout(&app, "bob", None, CLOUD_OFFER).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let reservation:i64=sqlx::query_scalar("SELECT count(*) FROM api_stripe_customers WHERE account_id='user-bob' AND customer_id IS NULL AND generation=0").fetch_one(&pool).await.unwrap();
    assert_eq!(reservation, 1);
    assert_eq!(
        post_checkout(&app, "bob", None, CLOUD_OFFER).await.0,
        StatusCode::OK
    );
    assert_eq!(fixture.keys.lock().unwrap().len(), 1);
    assert!(
        fixture
            .keys
            .lock()
            .unwrap()
            .contains_key("zuse-customer:user-bob")
    );
    // Complete the new purchase through the real signed canonical subscription event path.
    let now = chrono::Utc::now().timestamp_millis();
    assert_eq!(activate(&store, "sub-new").await["duplicate"], false);
    assert_eq!(
        post_checkout(&app, "alice", None, CLOUD_OFFER).await.1["error"],
        "machine_subscription_exists"
    );
    let old_id: String = sqlx::query_scalar(
        "SELECT customer_id FROM api_stripe_customers WHERE account_id='user-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(old_id, "cus_original");
    let old_sub: String = sqlx::query_scalar(
        "SELECT provider_subscription_id FROM api_entitlements WHERE entitlement_id='ent-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(old_sub, "sub-preserved");
    // A reservation older than Stripe's idempotency window must finish remote discovery first.
    sqlx::query("INSERT INTO api_stripe_customers(account_id,created_at) VALUES('user-recovery',$1),('user-absent',$1),('user-ambiguous',$1)").bind(now-24*3600000).execute(&pool).await.unwrap();
    fixture.list_mode.store(1, Ordering::SeqCst);
    let creates = fixture.creates.load(Ordering::SeqCst);
    assert_eq!(
        stripe_customer(&pool, stripe.as_ref(), "user-recovery", now)
            .await
            .unwrap_err()
            .code,
        "billing_customer_reconciliation_required"
    );
    assert_eq!(fixture.creates.load(Ordering::SeqCst), creates);
    assert_eq!(
        stripe_customer(&pool, stripe.as_ref(), "user-recovery", now)
            .await
            .unwrap(),
        "cus_recovered"
    );
    assert_eq!(fixture.creates.load(Ordering::SeqCst), creates);
    fixture.list_mode.store(0, Ordering::SeqCst);
    assert_eq!(
        stripe_customer(&pool, stripe.as_ref(), "user-absent", now)
            .await
            .unwrap(),
        "cus_user-absent"
    );
    assert!(
        fixture
            .keys
            .lock()
            .unwrap()
            .contains_key("zuse-customer:user-absent:generation:1")
    );
    fixture.list_mode.store(2, Ordering::SeqCst);
    let creates = fixture.creates.load(Ordering::SeqCst);
    assert_eq!(
        stripe_customer(&pool, stripe.as_ref(), "user-ambiguous", now)
            .await
            .unwrap_err()
            .code,
        "billing_customer_reconciliation_required"
    );
    assert_eq!(fixture.creates.load(Ordering::SeqCst), creates);
    fixture.list_mode.store(0, Ordering::SeqCst);
    if let Ok(path) = std::env::var("ZUSE_CLOUD_CHECKOUT_UI_FIXTURE_FILE") {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let api = tokio::spawn(axum::serve(listener, app).into_future());
        let file = std::path::Path::new(&path);
        std::fs::write(file, origin).unwrap();
        tokio::time::timeout(Duration::from_secs(900), async {
            let mut activated = false;
            while !file.with_extension("done").exists() {
                if !activated && file.with_extension("activate").exists() {
                    assert_eq!(activate(&store, "sub-ui").await["duplicate"], false);
                    activated = true;
                    std::fs::write(file.with_extension("activated"), "verified").unwrap();
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(
                activated,
                "UI acceptance must refresh after signed activation"
            );
        })
        .await
        .expect("Checkout UI acceptance timed out");
        assert!(
            fixture
                .forms
                .lock()
                .unwrap()
                .iter()
                .filter(|form| form["client_reference_id"] == "user-ui")
                .count()
                >= 2,
            "Native and browser must create real checkout sessions"
        );
        api.abort();
    }
    server.abort();
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
async fn polar_checkout_and_stripe_subscription_only_preserve_provider_parameters() {
    async fn polar(h: HeaderMap, Json(value): Json<Value>) -> Json<Value> {
        authorized(&h);
        assert_eq!(value["external_customer_id"], "organization:org-team");
        assert_eq!(value["products"], json!(["product_cloud"]));
        assert_eq!(value["allow_trial"], false);
        assert_eq!(value["metadata"]["offer_id"], CLOUD_OFFER);
        assert_eq!(
            value["success_url"],
            "https://api-rust.zuse.sh/complete?checkout_id={CHECKOUT_ID}"
        );
        Json(json!({"url":"https://polar.example.test/checkout"}))
    }
    async fn stripe(h: HeaderMap, Form(form): Form<HashMap<String, String>>) -> Json<Value> {
        authorized(&h);
        assert_eq!(form["line_items[0][price]"], "price_cloud");
        assert!(!form.contains_key("line_items[1][price]"));
        assert_eq!(form["automatic_tax[enabled]"], "false");
        assert_eq!(form["customer"], "cus_original");
        Json(json!({"url":"https://stripe.example.test/checkout"}))
    }
    async fn customer(h: HeaderMap) -> Json<Value> {
        authorized(&h);
        Json(json!({"id":"cus_original","metadata":{"account_id":"user-alice"}}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/checkouts/", post(polar))
                .route("/checkout/sessions", post(stripe))
                .route("/customers/cus_original", get(customer)),
        )
        .into_future(),
    );
    let http = || ProviderHttp {
        client: Client::new(),
        origin: Url::parse(&origin).unwrap(),
        secret: Zeroizing::new("fixture-key".into()),
    };
    let polar = PolarPortal {
        http: http(),
        events: None,
        checkout: Some(ProviderCheckout {
            offers: HashMap::from([(CLOUD_OFFER.into(), "product_cloud".into())]),
            overage: None,
            automatic_tax: true,
        }),
    };
    assert_eq!(
        polar
            .checkout(
                "organization:org-team",
                None,
                CLOUD_OFFER,
                "https://api-rust.zuse.sh/complete?checkout_id={CHECKOUT_ID}"
            )
            .await
            .unwrap(),
        "https://polar.example.test/checkout"
    );
    let stripe = StripePortal {
        http: http(),
        events: None,
        checkout: Some(ProviderCheckout {
            offers: HashMap::from([(CLOUD_OFFER.into(), "price_cloud".into())]),
            overage: None,
            automatic_tax: false,
        }),
        return_url: "https://api-rust.zuse.sh".into(),
        configuration: None,
    };
    assert_eq!(
        stripe
            .checkout(
                "user-alice",
                Some("cus_original"),
                CLOUD_OFFER,
                "https://api-rust.zuse.sh/complete?checkout_id={CHECKOUT_ID}"
            )
            .await
            .unwrap(),
        "https://stripe.example.test/checkout"
    );
    server.abort();
}
