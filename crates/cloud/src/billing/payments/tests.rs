use super::*;
use axum::{
    Json, Router,
    extract::{Form, State},
    http::HeaderMap,
    routing::{get, post},
};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Fixture {
    owner: std::sync::Mutex<String>,
    sessions: AtomicUsize,
}
async fn customer(State(f): State<Arc<Fixture>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers["authorization"], "Bearer fixture-payment-key");
    assert_eq!(headers["stripe-version"], STRIPE_API_VERSION);
    Json(json!({"id":"cus_original","metadata":{"account_id":f.owner.lock().unwrap().clone()}}))
}
async fn portal(
    State(f): State<Arc<Fixture>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(headers["authorization"], "Bearer fixture-payment-key");
    assert_eq!(headers["stripe-version"], STRIPE_API_VERSION);
    assert_eq!(form["customer"], "cus_original");
    assert_eq!(form["return_url"], "https://api-rust.zuse.sh");
    assert_eq!(form["configuration"], "bpc_original");
    f.sessions.fetch_add(1, Ordering::SeqCst);
    Json(json!({"customer":"cus_original","url":"https://billing.stripe.com/p/session/fixture"}))
}
#[tokio::test]
async fn stripe_portal_verifies_original_customer_before_any_session() {
    let fixture = Arc::new(Fixture {
        owner: std::sync::Mutex::new("owner".into()),
        sessions: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/v1/customers/cus_original", get(customer))
        .route("/v1/billing_portal/sessions", post(portal))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/v1/", listener.local_addr().unwrap());
    let task = tokio::spawn(axum::serve(listener, app).into_future());
    let mut http =
        ProviderHttp::new("https://api.stripe.com/v1/", "fixture-payment-key".into()).unwrap();
    http.origin = Url::parse(&origin).unwrap();
    let stripe = StripePortal {
        meter_price: None,
        credit_price: None,
        checkout: None,
        events: None,
        http,
        return_url: "https://api-rust.zuse.sh".into(),
        configuration: Some("bpc_original".into()),
    };
    assert!(stripe.portal("owner", None).await.is_err());
    assert_eq!(fixture.sessions.load(Ordering::SeqCst), 0);
    assert!(
        stripe
            .portal("foreign", Some("cus_original"))
            .await
            .is_err()
    );
    assert_eq!(fixture.sessions.load(Ordering::SeqCst), 0);
    assert_eq!(
        stripe.portal("owner", Some("cus_original")).await.unwrap(),
        "https://billing.stripe.com/p/session/fixture"
    );
    assert_eq!(fixture.sessions.load(Ordering::SeqCst), 1);
    task.abort();
}
