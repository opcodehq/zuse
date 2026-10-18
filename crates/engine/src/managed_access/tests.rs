use super::*;
use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::Jwk};
use std::{
    collections::HashSet,
    sync::{
        Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::sync::Notify;
#[derive(Default)]
struct Fixture {
    origin: String,
    mints: AtomicUsize,
    revoked: AtomicUsize,
    refuse_once: AtomicBool,
    hold: AtomicBool,
    entered: Notify,
    release: Notify,
    nonces: StdMutex<HashSet<String>>,
    subjects: StdMutex<Vec<String>>,
}
fn validate_proof(
    headers: &HeaderMap,
    method: &Method,
    url: &str,
    access: Option<&str>,
    state: &Fixture,
) -> Value {
    let token = headers["dpop"].to_str().unwrap();
    let header = jsonwebtoken::decode_header(token).unwrap();
    assert_eq!(header.alg, Algorithm::ES256);
    assert_eq!(header.typ.as_deref(), Some("dpop+jwt"));
    let mut validation = Validation::new(Algorithm::ES256);
    validation.required_spec_claims.clear();
    validation.validate_exp = false;
    validation.validate_aud = false;
    let key = DecodingKey::from_jwk(&header.jwk.unwrap()).unwrap();
    let proof = jsonwebtoken::decode::<Value>(token, &key, &validation)
        .unwrap()
        .claims;
    assert_eq!(proof["htu"], url);
    assert_eq!(proof["htm"], method.as_str());
    assert!(
        state
            .nonces
            .lock()
            .unwrap()
            .insert(proof["jti"].as_str().unwrap().into())
    );
    assert!((chrono::Utc::now().timestamp() - proof["iat"].as_i64().unwrap()).abs() < 10);
    if let Some(access) = access {
        use sha2::{Digest, Sha256};
        assert_eq!(
            proof["ath"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(access.as_bytes()))
        );
    }
    proof
}
async fn api(
    State(state): State<Arc<Fixture>>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path();
    let authorization = headers["authorization"].to_str().unwrap();
    if let Some(token) = authorization.strip_prefix("Bearer ") {
        let payload: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(token.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        state
            .subjects
            .lock()
            .unwrap()
            .push(payload["sub"].as_str().unwrap().into());
    }
    let value = match (method.clone(), path) {
        (Method::GET, "/v1/environments") => {
            if state.hold.load(Ordering::SeqCst) {
                state.entered.notify_one();
                state.release.notified().await;
            }
            json!({"environments":[{"environmentId":"env_remote","label":"Build computer","providerKind":"desktop","linkedAt":1,"endpoint":{"httpBaseUrl":"https://computer.example","wsBaseUrl":"wss://computer.example"}}]})
        }
        (Method::GET, "/v1/clients") => json!({"clients":[]}),
        (Method::POST, "/v1/client/environment-link-challenges") => {
            json!({"challengeId":"challenge_1","challenge":"opaque-challenge","apiIssuer":"https://account.example","expiresAt":chrono::Utc::now().timestamp_millis()+60000})
        }
        (Method::POST, "/v1/client/environment-links") => {
            let body: Value = serde_json::from_slice(&body).unwrap();
            let jwk: Jwk =
                serde_json::from_str(body["environmentPublicKey"].as_str().unwrap()).unwrap();
            let mut validation = Validation::new(Algorithm::EdDSA);
            validation.set_audience(&["https://account.example"]);
            let proof = jsonwebtoken::decode::<Value>(
                body["proof"].as_str().unwrap(),
                &DecodingKey::from_jwk(&jwk).unwrap(),
                &validation,
            )
            .unwrap()
            .claims;
            assert_eq!(proof["challenge"], "opaque-challenge");
            assert_eq!(proof["environmentId"], body["environmentId"]);
            assert_eq!(body["origin"]["localHttpHost"], "127.0.0.1");
            assert_eq!(body["endpoint"]["httpBaseUrl"], "http://127.0.0.1:27656");
            json!({"environmentId":body["environmentId"],"endpoint":{"httpBaseUrl":"https://computer.example","wsBaseUrl":"wss://computer.example"},"apiIssuer":"https://account.example","environmentCredential":"private-host-credential","mintPublicKey":body["environmentPublicKey"],"tunnelHostname":"computer.example","connectorToken":"private-connector"})
        }
        (Method::POST, "/v1/client/dpop-token") => {
            validate_proof(
                &headers,
                &method,
                &format!("{}{path}", state.origin),
                None,
                &state,
            );
            let index = state.mints.fetch_add(1, Ordering::SeqCst) + 1;
            json!({"accessToken":format!("grant-{index}"),"expiresIn":3600000})
        }
        (Method::POST, "/v1/mobile/devices") => {
            let access = authorization.strip_prefix("DPoP ").unwrap();
            validate_proof(
                &headers,
                &method,
                &format!("{}{path}", state.origin),
                Some(access),
                &state,
            );
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["platform"], "desktop");
            assert!(body["dpopJwk"]["d"].is_null());
            json!({"ok":true})
        }
        (Method::POST, "/v1/environments/env_remote/connect") => {
            let access = authorization.strip_prefix("DPoP ").unwrap();
            validate_proof(
                &headers,
                &method,
                &format!("{}{path}", state.origin),
                Some(access),
                &state,
            );
            if state.refuse_once.swap(false, Ordering::SeqCst) {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["wireProtocolVersion"], 1);
            assert_eq!(body["requireManaged"], true);
            json!({"endpoint":{"httpBaseUrl":"https://computer.example","wsBaseUrl":"wss://computer.example"},"connectToken":"private-bootstrap","expiresAt":chrono::Utc::now().timestamp_millis()+60000})
        }
        (Method::DELETE, "/v1/clients/device_1") => {
            state.revoked.fetch_add(1, Ordering::SeqCst);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        (Method::DELETE, "/v1/clients/redirect") => {
            return (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", format!("{}/v1/clients/device_1", state.origin))],
            )
                .into_response();
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    axum::Json(value).into_response()
}
async fn fixture() -> (Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let state = Arc::new(Fixture {
        origin: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    });
    let app = Router::new().fallback(api).with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (state, task)
}
#[tokio::test]
async fn account_discovery_link_proofs_shared_renewal_and_no_uncertain_write_replay() {
    let root = tempfile::tempdir().unwrap();
    let (state, server) = fixture().await;
    let account = Account::fixture_signed_in(root.path(), state.origin.clone(), "user_a");
    let client = AccountClient::new(account).unwrap();
    assert_eq!(
        client.environments().await.unwrap().environments[0].environment_id,
        "env_remote"
    );
    assert!(client.clients().await.unwrap().clients.is_empty());
    let identity = HostIdentity::open(root.path(), "device_1").unwrap();
    let public = identity.public_jwk();
    let receipt = client
        .link(&identity, "Build computer", 27656)
        .await
        .unwrap();
    assert_eq!(receipt.environment_id, "device_1");
    assert!(receipt.connector_token.is_some());
    drop(identity);
    assert_eq!(
        HostIdentity::open(root.path(), "device_1")
            .unwrap()
            .public_jwk(),
        public
    );
    assert!(matches!(
        HostIdentity::open(root.path(), "other_device"),
        Err(Error::Identity)
    ));
    state.refuse_once.store(true, Ordering::SeqCst);
    let jobs = (0..12)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move {
                client
                    .connect("env_remote", "device_1", 1)
                    .await
                    .unwrap()
                    .connect_token
            })
        })
        .collect::<Vec<_>>();
    for job in jobs {
        assert_eq!(job.await.unwrap(), "private-bootstrap");
    }
    assert_eq!(state.mints.load(Ordering::SeqCst), 2);
    assert!(matches!(
        client.revoke_client("device_1").await,
        Err(Error::Unavailable)
    ));
    assert_eq!(state.revoked.load(Ordering::SeqCst), 1);
    assert!(matches!(
        client.revoke_client("redirect").await,
        Err(Error::Rejected)
    ));
    assert_eq!(state.revoked.load(Ordering::SeqCst), 1);
    assert!(matches!(
        client.revoke_client("../escape").await,
        Err(Error::Invalid)
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.path().join("managed-access/identity.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn account_change_cancels_in_flight_reads_and_retires_cached_proof_credentials() {
    let root = tempfile::tempdir().unwrap();
    let (state, server) = fixture().await;
    let account = Account::fixture_signed_in(root.path(), state.origin.clone(), "user_a");
    let client = AccountClient::new(account.clone()).unwrap();
    client.connect("env_remote", "device_1", 1).await.unwrap();
    assert_eq!(state.mints.load(Ordering::SeqCst), 1);
    state.hold.store(true, Ordering::SeqCst);
    let copy = client.clone();
    let pending = tokio::spawn(async move { copy.environments().await });
    state.entered.notified().await;
    account.sign_out().unwrap();
    assert!(matches!(pending.await.unwrap(), Err(Error::Changed)));
    state.release.notify_waiters();
    state.hold.store(false, Ordering::SeqCst);
    assert!(matches!(client.environments().await, Err(Error::SignedOut)));
    account.fixture_install("user_b");
    client.connect("env_remote", "device_1", 1).await.unwrap();
    assert_eq!(state.mints.load(Ordering::SeqCst), 2);
    assert!(state.subjects.lock().unwrap().contains(&"user_b".into()));
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn engine_account_discovery_rejects_foreign_targets_without_breaking_engine_rpc() {
    use zuse_rpc::methods;
    let root = tempfile::tempdir().unwrap();
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(root.path()).unwrap(),
        Arc::new(crate::HarnessRegistry::new()),
        zuse_protocol::HarnessId::Mock,
        None,
    )
    .unwrap();
    let rpc = zuse_rpc::memory_client(core.rpc_service());
    for method in [
        methods::LIST_ACCOUNT_ENVIRONMENTS,
        methods::LIST_ACCOUNT_DEVICES,
        methods::REVOKE_ACCOUNT_DEVICE,
    ] {
        assert!(
            rpc.call(
                method,
                json!({"targetDeviceId":"foreign","clientId":"device_1"})
            )
            .await
            .is_err()
        );
    }
    assert!(
        rpc.call(methods::LIST_ACCOUNT_ENVIRONMENTS, json!({}))
            .await
            .is_err()
    );
    assert!(rpc.call(methods::ENGINE_INFO, json!({})).await.is_ok());
    assert!(methods::retryable_read(
        methods::LIST_ACCOUNT_ENVIRONMENTS,
        &json!({})
    ));
    assert!(!methods::retryable_read(
        methods::REVOKE_ACCOUNT_DEVICE,
        &json!({})
    ));
    core.shutdown().await;
}
