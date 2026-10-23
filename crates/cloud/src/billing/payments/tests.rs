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
    Json(json!({"id":"cus_original","metadata":{"account_id":f.owner.lock().unwrap().clone()}}))
}
async fn portal(
    State(f): State<Arc<Fixture>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(headers["authorization"], "Bearer fixture-payment-key");
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
    let stripe = StripePortal {
        http: ProviderHttp {
            client: Client::new(),
            origin: Url::parse(&origin).unwrap(),
            secret: Zeroizing::new("fixture-payment-key".into()),
        },
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

#[tokio::test]
async fn polar_portal_uses_original_external_owner_and_rejects_unsafe_links() {
    async fn session(Json(body): Json<Value>) -> Json<Value> {
        let owner = body["external_customer_id"].as_str().unwrap();
        assert_eq!(body.as_object().unwrap().len(), 1);
        Json(
            json!({"customer_portal_url":if owner=="organization:org-team" {"https://polar.sh/portal/fixture"}else{"http://unsafe.example.test/portal"}}),
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/v1/", listener.local_addr().unwrap());
    let task = tokio::spawn(
        axum::serve(
            listener,
            Router::new().route("/v1/customer-sessions/", post(session)),
        )
        .into_future(),
    );
    let polar = PolarPortal {
        http: ProviderHttp {
            client: Client::new(),
            origin: Url::parse(&origin).unwrap(),
            secret: Zeroizing::new("fixture-polar-token".into()),
        },
    };
    assert_eq!(
        polar.portal("organization:org-team", None).await.unwrap(),
        "https://polar.sh/portal/fixture"
    );
    assert!(polar.portal("user-other", None).await.is_err());
    task.abort();
}
