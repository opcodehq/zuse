use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct Server {
    mode: String,
    calls: Vec<String>,
    renew_ids: Vec<String>,
    signing_x: String,
    origin: String,
}
type Shared = Arc<Mutex<Server>>;
async fn query(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(body, json!({"kind":"query"}));
    let mut s = s.lock().unwrap();
    s.calls.push("query".into());
    let renewed = s.calls.iter().any(|c| c == "renew");
    assert_eq!(
        headers["authorization"],
        if renewed {
            "Bearer renewed-bearer"
        } else {
            "Bearer original-bearer"
        }
    );
    match s.mode.as_str() {
        "transient-query-exhausted" | "transient-query"
            if s.mode == "transient-query-exhausted" || s.calls.len() == 1 =>
        {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"unavailable"})),
            );
        }
        "revoked" => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error":"workspace_runtime_revoked"})),
            );
        }
        "deleted" | "withdrawn" | "renewed-denied" => {
            return (StatusCode::OK, Json(json!({"directive":null})));
        }
        "rejected" if !renewed => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"workspace_runtime_fenced"})),
            );
        }
        _ => {}
    }
    let mut d = json!({"operationId":"resume-original","action":"resume","phase":"launching","generation":7,"storageIncarnationId":"disk-original","sourceMachineId":"machine-original"});
    match s.mode.as_str() {
        "wrong-op" => d["operationId"] = json!("other-op"),
        "wrong-gen" => d["generation"] = json!(8),
        "wrong-disk" => d["storageIncarnationId"] = json!("other-disk"),
        "wrong-action" => d["action"] = json!("pause"),
        "wrong-phase" => d["phase"] = json!("quiescing"),
        _ => {}
    }
    (StatusCode::OK, Json(json!({"directive":d})))
}
async fn renewal(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut s = s.lock().unwrap();
    assert_eq!(headers["authorization"], "Bearer original-bearer");
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
    validation.set_audience(&[s.origin.as_str()]);
    let claims = jsonwebtoken::decode::<Value>(
        body["proof"].as_str().unwrap(),
        &jsonwebtoken::DecodingKey::from_ed_components(&s.signing_x).unwrap(),
        &validation,
    )
    .unwrap()
    .claims;
    assert_eq!(claims["workspaceId"], "workspace");
    assert_eq!(claims["generation"], 7);
    assert_eq!(claims["gatewayEpoch"], 3);
    assert_eq!(claims["requestId"], body["requestId"]);
    s.calls.push("renew".into());
    s.renew_ids.push(body["requestId"].as_str().unwrap().into());
    if s.mode == "renew-revoked" {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"workspace_runtime_revoked"})),
        );
    }
    if s.mode == "transient-renewal" && s.renew_ids.len() == 1 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"unavailable"})),
        );
    }
    (
        StatusCode::OK,
        Json(
            json!({"workspaceId":"workspace","requestId":body["requestId"],"generation":7,"gatewayEpoch":3,"runtimeCredential":"renewed-bearer","expiresAt":crate::now_ms()+3_600_000}),
        ),
    )
}
fn sealed(identity: &Identity) -> String {
    use aes_gcm::{
        Aes256Gcm, KeyInit, Nonce,
        aead::{Aead, Payload},
    };
    use rsa::{BigUint, Oaep, RsaPublicKey};
    use sha2::Sha256;
    let public: Value = serde_json::from_str(&identity.request().credential_public_jwk).unwrap();
    let key = RsaPublicKey::new(
        BigUint::from_bytes_be(&B64.decode(public["n"].as_str().unwrap()).unwrap()),
        BigUint::from_bytes_be(&B64.decode(public["e"].as_str().unwrap()).unwrap()),
    )
    .unwrap();
    let head =
        B64.encode(serde_json::to_vec(&json!({"alg":"RSA-OAEP-256","enc":"A256GCM"})).unwrap());
    let cek = [17; 32];
    let iv = [18; 12];
    let wrapped = key
        .encrypt(&mut rand::thread_rng(), Oaep::new::<Sha256>(), &cek)
        .unwrap();
    let encrypted = Aes256Gcm::new_from_slice(&cek)
        .unwrap()
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: B64.encode([19; 32]).as_bytes(),
                aad: head.as_bytes(),
            },
        )
        .unwrap();
    let (cipher, tag) = encrypted.split_at(encrypted.len() - 16);
    format!(
        "{head}.{}.{}.{}.{}",
        B64.encode(wrapped),
        B64.encode(iv),
        B64.encode(cipher),
        B64.encode(tag)
    )
}

#[tokio::test]
async fn valid_memory_bearer_reuses_only_after_fresh_http_operation_authority() {
    let shared = Shared::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/v1/cloud/workspaces/workspace/runtime/lifecycle",
            post(query),
        )
        .route(
            "/v1/cloud/workspaces/workspace/runtime/credentials/renew",
            post(renewal),
        )
        .with_state(shared.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let temp = tempfile::tempdir().unwrap();
    let binding = crate::IdentityBinding {
        workspace: "workspace".into(),
        api_url: origin.clone(),
        incarnation: "disk-original".into(),
        generation: 7,
    };
    let mut identity = Identity::open(temp.path(), &binding, Some("boot-token")).unwrap();
    let seal = sealed(&identity);
    let boot:CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"workspace","zuseAccountId":"owner","chatId":"chat","initialSessionId":"session","runtimeCredential":"original-bearer","runtimeGeneration":7,"gatewayEpoch":3,"runtimeCredentialExpiresAt":crate::now_ms()+3_600_000,"gatewayUrl":"ws://localhost/gateway","gatewayProtocol":"zuse-rust-rpc-v1","sealedTranscriptKey":seal,"providerSandboxId":"machine-original","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk-original"}})).unwrap();
    let public: Value = serde_json::from_str(&identity.request().signing_public_jwk).unwrap();
    {
        let mut s = shared.lock().unwrap();
        s.signing_x = public["x"].as_str().unwrap().into();
        s.origin = origin.clone();
    }
    let authority = Authority::new(&origin).unwrap();
    let wake = Wake {
        workspace_id: "workspace".into(),
        generation: 7,
        storage_incarnation_id: "disk-original".into(),
        operation_id: "resume-original".into(),
    };
    for (mode, margin, pending, allowed, expected) in [
        ("valid", 3_600_000, false, true, vec!["query"]),
        (
            "transient-query",
            3_600_000,
            false,
            true,
            vec!["query", "query"],
        ),
        (
            "transient-query-exhausted",
            3_600_000,
            false,
            false,
            vec!["query", "query", "query"],
        ),
        ("near", 30_000, false, true, vec!["renew", "query"]),
        ("expired", -1, false, true, vec!["renew", "query"]),
        ("pending", 3_600_000, true, true, vec!["renew", "query"]),
        (
            "transient-renewal",
            3_600_000,
            true,
            true,
            vec!["renew", "renew", "query"],
        ),
        (
            "rejected",
            3_600_000,
            false,
            true,
            vec!["query", "renew", "query"],
        ),
        ("revoked", 3_600_000, false, false, vec!["query"]),
        ("deleted", 3_600_000, false, false, vec!["query"]),
        ("withdrawn", 3_600_000, false, false, vec!["query"]),
        ("wrong-op", 3_600_000, false, false, vec!["query"]),
        ("wrong-gen", 3_600_000, false, false, vec!["query"]),
        ("wrong-disk", 3_600_000, false, false, vec!["query"]),
        ("wrong-action", 3_600_000, false, false, vec!["query"]),
        ("wrong-phase", 3_600_000, false, false, vec!["query"]),
        ("renewed-denied", -1, false, false, vec!["renew", "query"]),
        ("renew-revoked", -1, false, false, vec!["renew"]),
    ] {
        let mut original = boot.clone();
        original.runtime_credential_expires_at = crate::now_ms() + margin;
        identity.commit_bootstrap(original, 7).unwrap();
        let request = if pending {
            let request = identity.begin_renewal().unwrap();
            identity = Identity::open(temp.path(), &binding, None).unwrap();
            assert!(identity.has_pending_renewal());
            Some(request)
        } else {
            None
        };
        {
            let mut s = shared.lock().unwrap();
            s.mode = mode.into();
            s.calls.clear();
            s.renew_ids.clear();
        }
        let result = authorize(&authority, &mut identity, &wake, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.is_some(), allowed, "{mode}");
        let s = shared.lock().unwrap();
        assert_eq!(s.calls, expected, "{mode}");
        if let Some(request) = request {
            assert!(
                s.renew_ids.iter().all(|r| r == &request),
                "{mode}: original request must survive retry"
            );
        }
        if let Some((current, d)) = result {
            assert_eq!(d.operation_id, wake.operation_id);
            assert_eq!(
                current.runtime_credential,
                if s.calls.iter().any(|c| c == "renew") {
                    "renewed-bearer"
                } else {
                    "original-bearer"
                }
            );
        }
    }
    identity.commit_bootstrap(boot, 7).unwrap();
    shared.lock().unwrap().calls.clear();
    for altered in [
        Wake {
            workspace_id: "other".into(),
            ..wake.clone()
        },
        Wake {
            generation: 8,
            ..wake.clone()
        },
        Wake {
            storage_incarnation_id: "other".into(),
            ..wake.clone()
        },
    ] {
        assert!(
            authorize(
                &authority,
                &mut identity,
                &altered,
                &CancellationToken::new()
            )
            .await
            .unwrap()
            .is_none()
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        authorize(&authority, &mut identity, &wake, &cancel)
            .await
            .unwrap()
            .is_none()
    );
    assert!(shared.lock().unwrap().calls.is_empty());
    server.abort();
}

#[tokio::test]
async fn transient_wake_retries_are_bounded_cancelled_and_expiry_fenced() {
    let transient = anyhow::Error::new(crate::authority::Failure {
        code: "unavailable",
        retryable: true,
    });
    let rejected = anyhow::Error::new(crate::authority::Failure {
        code: "workspace_runtime_revoked",
        retryable: false,
    });
    let cancel = CancellationToken::new();
    let mut retries = 0;
    assert!(!retry_transient(&rejected, &mut retries, crate::now_ms() + 60_000, &cancel).await);
    assert_eq!(retries, 0);
    assert!(!retry_transient(&transient, &mut retries, crate::now_ms() - 1, &cancel).await);
    assert_eq!(retries, 0);
    let cancel_wait = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cancel_wait.cancel();
    });
    assert!(!retry_transient(&transient, &mut retries, crate::now_ms() + 60_000, &cancel).await);
    assert_eq!(retries, 1);
    let cancel = CancellationToken::new();
    let mut retries = 2;
    assert!(!retry_transient(&transient, &mut retries, crate::now_ms() + 60_000, &cancel).await);
    assert_eq!(retries, 2);
    let mut retries = 0;
    assert!(!retry_transient(&transient, &mut retries, crate::now_ms() + 1, &cancel).await);
    assert_eq!(retries, 1);
}
