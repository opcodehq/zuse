use super::{ProviderError, boat::Boat, contract::*};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
struct Fixture {
    status: Arc<Mutex<String>>,
    stops: Arc<AtomicUsize>,
    creates: Arc<AtomicUsize>,
    leaks: Arc<AtomicUsize>,
}
fn auth(headers: &HeaderMap) {
    assert_eq!(headers["authorization"], "Bearer test-private-key");
}
async fn inspect(headers: HeaderMap) -> Json<Value> {
    auth(&headers);
    Json(json!({"sandbox":{"id":"sandbox-one","name":"task-one","state":"archived"}}))
}
async fn stop(State(fixture): State<Fixture>, headers: HeaderMap) -> Json<Value> {
    auth(&headers);
    fixture.stops.fetch_add(1, Ordering::SeqCst);
    Json(
        json!({"sandbox":{"id":"sandbox-one","state":"archiving"},"stop":{"id":"stop-original","status":"pending"}}),
    )
}
async fn operation(
    State(fixture): State<Fixture>,
    Path((id, operation)): Path<(String, String)>,
    headers: HeaderMap,
) -> Json<Value> {
    auth(&headers);
    assert_eq!(id, "sandbox-one");
    assert_eq!(operation, "stop-original");
    Json(json!({"stop":{"id":operation,"status":fixture.status.lock().unwrap().as_str()}}))
}
async fn create(
    State(fixture): State<Fixture>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    auth(&headers);
    fixture.creates.fetch_add(1, Ordering::SeqCst);
    assert_eq!(headers["idempotency-key"], "operation-one");
    assert_eq!(body["noEnv"], true);
    assert_eq!(body["from"], "managed-base");
    assert_eq!(body["name"], "task-one");
    assert_eq!(
        body["labels"],
        json!({"zuse-source-artifact":"managed-base","zuse-allocation-name":"task-one","zuse-allocation-operation":"operation-one"})
    );
    // The provider may have allocated before returning a malformed/truncated body.
    (StatusCode::ACCEPTED, "{truncated")
}
async fn redirect() -> impl IntoResponse {
    (StatusCode::TEMPORARY_REDIRECT, [("location", "/leak")])
}
async fn leak(State(fixture): State<Fixture>) {
    fixture.leaks.fetch_add(1, Ordering::SeqCst);
}

#[tokio::test]
async fn stop_receipt_survives_restart_and_archived_state_never_completes_a_pending_stop() {
    let fixture = Fixture::default();
    *fixture.status.lock().unwrap() = "pending".into();
    let app = Router::new()
        .route("/api/v1/sandboxes/sandbox-one", get(inspect))
        .route("/api/v1/sandboxes/sandbox-one/stop", post(stop))
        .route("/api/v1/sandboxes/{id}/stops/{operation}", get(operation))
        .route("/api/v1/sandboxes", post(create))
        .route("/api/v1/sandboxes/redirect", get(redirect))
        .route("/leak", get(leak))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let mut provider = Boat::new("test-private-key".into()).unwrap();
    provider.rest.test_endpoint(endpoint.clone());
    let receipt = provider.sleep("sandbox-one").await.unwrap();
    let persisted = serde_json::to_vec(&receipt).unwrap();
    drop(provider);
    let mut restarted = Boat::new("test-private-key".into()).unwrap();
    restarted.rest.test_endpoint(endpoint);
    let receipt = serde_json::from_slice(&persisted).unwrap();
    assert_eq!(
        restarted.operation(&receipt).await.unwrap(),
        OperationState::Pending
    );
    *fixture.status.lock().unwrap() = "failing".into();
    assert_eq!(
        restarted.operation(&receipt).await.unwrap(),
        OperationState::Retrying
    );
    *fixture.status.lock().unwrap() = "superseded".into();
    assert_eq!(
        restarted.operation(&receipt).await.unwrap(),
        OperationState::Superseded
    );
    *fixture.status.lock().unwrap() = "completed".into();
    assert_eq!(
        restarted.operation(&receipt).await.unwrap(),
        OperationState::Completed
    );
    assert_eq!(fixture.stops.load(Ordering::SeqCst), 1);
    let request = Allocation {
        artifact: "managed-base",
        artifact_binding: None,
        name: "task-one",
        operation_id: "operation-one",
        size: "small",
        lifetime_seconds: 600,
    };
    assert_eq!(
        restarted.allocate(&request).await.unwrap_err(),
        ProviderError::OutcomeUnknown
    );
    assert_eq!(fixture.creates.load(Ordering::SeqCst), 1);
    assert_eq!(
        restarted.inspect("redirect").await.unwrap_err(),
        ProviderError::Unavailable
    );
    assert_eq!(fixture.leaks.load(Ordering::SeqCst), 0);
    drop(server);
}

#[tokio::test]
async fn incomplete_reconciliation_listing_is_not_treated_as_absence() {
    let app = Router::new().route(
        "/sandboxes",
        get(|| async { Json(json!({"sandboxes":[],"pageInfo":{"nextCursor":"repeated-cursor"}})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let mut provider = Boat::new("test-private-key".into()).unwrap();
    provider.rest.test_endpoint(endpoint);
    assert_eq!(
        provider.find_allocation("task-one").await.unwrap_err(),
        ProviderError::InvalidResponse
    );
    drop(server);
}

#[tokio::test]
async fn boat_runtime_readiness_is_original_id_hydration_or_answering_runtime_not_generic_running()
{
    let state = Arc::new(Mutex::new((
        StatusCode::OK,
        json!({"sandbox":{"id":"original","name":"original-name","state":"idle","hydrated":false}}),
    )));
    let calls = Arc::new(AtomicUsize::new(0));
    let answers = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handler_state = state.clone();
    let handler_calls = calls.clone();
    let handler_answers = answers.clone();
    let app = Router::new()
        .route(
            "/api/v1/sandboxes/original",
            get(move |headers: HeaderMap| {
                let state = handler_state.clone();
                let calls = handler_calls.clone();
                async move {
                    auth(&headers);
                    calls.fetch_add(1, Ordering::SeqCst);
                    let value = state.lock().unwrap().clone();
                    (value.0, Json(value.1))
                }
            }),
        )
        .route(
            "/api/v1/sandboxes/original/commands",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let answers = handler_answers.clone();
                async move {
                    auth(&headers);
                    assert_eq!(body["command"], super::boat::RUNTIME_ANSWERS);
                    let code = if answers.load(Ordering::SeqCst) { 0 } else { 1 };
                    Json(json!({"exitCode":code,"stdout":"","stderr":"","timedOut":false}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let mut provider = Boat::new("test-private-key".into()).unwrap();
    provider.rest.test_endpoint(endpoint);
    assert_eq!(
        provider.inspect("original").await.unwrap().unwrap().state,
        super::MachineState::Running,
        "generic running remains usable for stop reconciliation"
    );
    assert!(
        !provider.runtime_ready("original").await.unwrap(),
        "not hydrated and the installed runtime does not answer yet"
    );
    answers.store(true, Ordering::SeqCst);
    assert!(
        provider.runtime_ready("original").await.unwrap(),
        "a lazily restored disk that already runs the runtime is ready"
    );
    answers.store(false, Ordering::SeqCst);
    for (value, expected) in [
        (
            json!({"sandbox":{"id":"original","state":"idle","hydrated":true}}),
            Ok(true),
        ),
        (
            json!({"sandbox":{"id":"original","state":"idle"}}),
            Err(ProviderError::InvalidResponse),
        ),
        (
            json!({"sandbox":{"id":"original","state":"idle","hydrated":"true"}}),
            Err(ProviderError::InvalidResponse),
        ),
        (
            json!({"sandbox":{"id":"replacement","state":"idle","hydrated":true}}),
            Err(ProviderError::InvalidResponse),
        ),
        (
            json!({"sandbox":{"id":"original","state":"archived","hydrated":true}}),
            Ok(false),
        ),
        (
            json!({"sandbox":{"id":"original","state":"resuming","hydrated":false}}),
            Ok(false),
        ),
    ] {
        *state.lock().unwrap() = (StatusCode::OK, value);
        assert_eq!(provider.runtime_ready("original").await, expected);
    }
    *state.lock().unwrap() = (StatusCode::NOT_FOUND, json!({"error":"not-found"}));
    assert_eq!(
        provider.runtime_ready("original").await,
        Err(ProviderError::NotFound)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 10);
}

#[tokio::test]
async fn boat_fresh_boot_requires_original_source_labels_name_and_same_id() {
    let original = json!({"id":"original-id","name":"original-name","state":"idle","hydrated":true,
        "labels":{"zuse-source-artifact":"original-base","zuse-allocation-name":"original-name"}});
    let state = Arc::new(Mutex::new(original.clone()));
    let lookup = state.clone();
    let app = Router::new()
        .route(
            "/api/v1/saved-snapshots/original-base",
            get(|| async { Json(json!({"snapshot":{"name":"original-base","status":"ready"}})) }),
        )
        .route(
            "/api/v1/sandboxes/original-id",
            get(move || {
                let lookup = lookup.clone();
                async move { Json(json!({"sandbox":lookup.lock().unwrap().clone()})) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut provider = Boat::new("test-private-key".into()).unwrap();
    provider
        .rest
        .test_endpoint(format!("http://{}/api/v1", listener.local_addr().unwrap()));
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    provider
        .verify_initial_source("original-id", "original-base", "original-name")
        .await
        .unwrap();
    // Readiness (an answering runtime) is checked before this verification;
    // a disk still hydrating is accepted, a malformed response is not.
    let mut hydrating = original.clone();
    hydrating["hydrated"] = json!(false);
    *state.lock().unwrap() = hydrating;
    provider
        .verify_initial_source("original-id", "original-base", "original-name")
        .await
        .unwrap();
    for (path, value) in [
        ("id", json!("wrong-id")),
        ("name", json!("wrong-name")),
        ("hydrated", Value::Null),
        ("state", json!("archived")),
        (
            "labels",
            json!({"zuse-source-artifact":"wrong-base","zuse-allocation-name":"original-name"}),
        ),
        ("labels", Value::Null),
    ] {
        let mut changed = original.clone();
        changed[path] = value;
        *state.lock().unwrap() = changed;
        assert!(
            provider
                .verify_initial_source("original-id", "original-base", "original-name")
                .await
                .is_err(),
            "{path}"
        );
    }
}

#[tokio::test]
async fn boat_runtime_adapter_uses_scoped_helper_for_fresh_and_owned_resume_only_once_ready() {
    use crate::{
        managed_bases::bootstrap::{ImagePins, Purpose, Request},
        runtime::{
            allocation::AllocationProvider, lifecycle::RetainedProvider, provider::RuntimeProvider,
        },
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let hydrated = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let answers = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let commands = Arc::new(AtomicUsize::new(0));
    let hydration = hydrated.clone();
    let answering = answers.clone();
    let count = commands.clone();
    let app=Router::new()
      .route("/api/v1/saved-snapshots/original-base",get(||async {Json(json!({"snapshot":{"name":"original-base","status":"ready"}}))}))
      .route("/api/v1/sandboxes/original-id",get(move||{let h=hydration.clone();async move{Json(json!({"sandbox":{"id":"original-id","name":"original-name","state":"idle","hydrated":h.load(Ordering::SeqCst),"labels":{"zuse-source-artifact":"original-base","zuse-allocation-name":"original-name"}}}))}}))
      .route("/api/v1/sandboxes/original-id/commands",post(move|Json(v):Json<Value>|{let count=count.clone();let answering=answering.clone();async move {
        if v["command"]==super::boat::RUNTIME_ANSWERS {return Json(json!({"exitCode":if answering.load(Ordering::SeqCst){0}else{1},"stdout":"","stderr":"","timedOut":false}));}
        assert_eq!(v["timeoutSeconds"],60);let command=v["command"].as_str().unwrap();assert!(command.starts_with("sudo -n /usr/bin/python3 -I -B -c "));
        let body=command.rsplit('\'').nth(1).unwrap();let mut request:Value=serde_json::from_slice(&STANDARD.decode(body).unwrap()).unwrap();
        let typed:Request=serde_json::from_value(request.clone()).unwrap();assert!(typed.valid());assert_eq!(request["provider"],"boat");assert_eq!(request["sourceMachineId"],"original-id");
        let retained=typed.purpose==Purpose::RetainedResume;request["status"]=json!("ready");request["requestSha256"]=json!(typed.digest().unwrap());request["unitStartSubmitted"]=json!(!retained);
        request["resident"]=json!({"residentPid":42,"residentStartTicks":7,"bootNonceSha256":"a".repeat(64),"ipcVerified":true,"unbound":!retained,"identityVerified":retained,"childAlive":retained,"recoveryAlive":false});
        count.fetch_add(1,Ordering::SeqCst);Json(json!({"exitCode":0,"stdout":request.to_string(),"stderr":"","timedOut":false}))
      }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut boat = Boat::new("private-wire-key".into()).unwrap();
    boat.rest
        .test_endpoint(format!("http://{}/api/v1", listener.local_addr().unwrap()));
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let adapter = RuntimeProvider::new(Arc::new(boat), "small".into());
    let mut request = Request {
        version: 1,
        provider: zuse_protocol::cloud_computers::ProviderId::Boat,
        purpose: Purpose::InitialAllocation,
        operation_id: "original-operation".into(),
        workspace_id: "original-workspace".into(),
        generation: 1,
        storage_incarnation_id: "original-disk".into(),
        source_machine_id: "original-id".into(),
        image_pins: ImagePins {
            runtime_sha256: "a".repeat(64),
            supervisor_sha256: "b".repeat(64),
            service_sha256: "c".repeat(64),
            manifest_sha256: "d".repeat(64),
            helper_sha256: "e".repeat(64),
            runtime_update_helper_sha256: None,
        },
    };
    assert!(
        adapter
            .bootstrap_initial_runtime("original-id", "original-base", "original-name", &request)
            .await
            .unwrap()
            .is_some()
    );
    request.purpose = Purpose::RetainedResume;
    assert!(
        adapter
            .bootstrap_initial_runtime("original-id", "original-base", "original-name", &request)
            .await
            .is_err(),
        "fresh effect cannot accept retained authority"
    );
    assert!(adapter.bootstrap_retained_runtime(&request).await.unwrap());
    assert_eq!(commands.load(Ordering::SeqCst), 2);
    hydrated.store(false, Ordering::SeqCst);
    assert!(!adapter.bootstrap_retained_runtime(&request).await.unwrap());
    assert_eq!(
        commands.load(Ordering::SeqCst),
        2,
        "no helper effects before the restored disk runs the installed runtime"
    );
    answers.store(true, Ordering::SeqCst);
    assert!(adapter.bootstrap_retained_runtime(&request).await.unwrap());
    assert_eq!(
        commands.load(Ordering::SeqCst),
        3,
        "a hydrating disk that runs the installed runtime may resume"
    );
}

#[path = "boat_tests/failure_observations.rs"]
mod failure_observations;
