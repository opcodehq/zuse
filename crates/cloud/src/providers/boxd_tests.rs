use super::boxd::*;
use super::{MachineState, ProviderError};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Clone)]
struct Fixture {
    wire: Arc<Value>,
    exchanges: Arc<AtomicUsize>,
    creates: Arc<AtomicUsize>,
    truncate_create: Arc<AtomicBool>,
    reject: Arc<AtomicBool>,
}
fn decode(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}
fn response(fixture: &Fixture, names: &[&str], status: u32) -> Response<Body> {
    let mut bytes = Vec::new();
    for name in names {
        bytes.extend(frame(&decode(fixture.wire[name].as_str().unwrap())));
    }
    let mut trailer = frame(format!("grpc-status: {status}\r\n").as_bytes());
    trailer[0] = 0x80;
    bytes.extend(trailer);
    // Deliberately split inside headers and payloads; HTTP chunk boundaries are not RPC frames.
    let pieces = bytes
        .chunks(3)
        .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    Response::builder()
        .header("content-type", "application/grpc-web+proto")
        .body(Body::from_stream(futures::stream::iter(pieces)))
        .unwrap()
}
async fn exchange(
    State(fixture): State<Fixture>,
    axum::Json(body): axum::Json<Value>,
) -> axum::Json<Value> {
    assert_eq!(body, json!({"api_key":"fixture-api-key"}));
    fixture.exchanges.fetch_add(1, Ordering::Relaxed);
    axum::Json(
        json!({"token":"fixture-provider-token","expires_at":chrono::Utc::now().timestamp()+3600}),
    )
}
async fn rpc(
    State(fixture): State<Fixture>,
    Path(method): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    assert_eq!(headers["authorization"], "Bearer fixture-provider-token");
    assert_eq!(headers["content-type"], "application/grpc-web+proto");
    assert_eq!(headers["x-grpc-web"], "1");
    if fixture.reject.load(Ordering::Relaxed) {
        return Response::builder()
            .header("grpc-status", "16")
            .body(Body::empty())
            .unwrap();
    }
    let (request, replies) = match method.as_str() {
        "CreateVmFromSnapshot" => {
            fixture.creates.fetch_add(1, Ordering::Relaxed);
            if fixture.truncate_create.load(Ordering::Relaxed) {
                return Response::builder()
                    .body(Body::from(vec![0, 0, 0, 0, 9, 1]))
                    .unwrap();
            }
            ("createRequest", vec!["createResponse"])
        }
        "GetVm" => {
            if body.as_ref() == frame(&decode(fixture.wire["missingRequest"].as_str().unwrap())) {
                return response(&fixture, &[], 5);
            }
            ("machineRequest", vec!["machineResponse"])
        }
        "ResumeVm" => ("machineRequest", vec!["resumeResponse"]),
        "StartVm" | "HibernateVm" | "WakeVm" | "DestroyVm" => {
            ("machineRequest", vec!["emptyResponse"])
        }
        "SetAutoHibernateTimeout" => ("idleRequest", vec!["emptyResponse"]),
        "Exec" => ("execRequest", vec!["execOutput", "execError", "execExit"]),
        "UploadFile" => ("uploadRequest", vec!["uploadResponse"]),
        _ => panic!("Unexpected provider operation"),
    };
    assert_eq!(
        body.as_ref(),
        frame(&decode(fixture.wire[request].as_str().unwrap()))
    );
    response(&fixture, &replies, 0)
}

#[tokio::test]
async fn sdk_wire_matches_actual_http_lifecycle_bootstrap_and_ambiguous_create() {
    let fixture = Fixture {
        wire: Arc::new(
            serde_json::from_str(include_str!("../../tests/fixtures/boxd-wire.json")).unwrap(),
        ),
        exchanges: Arc::new(AtomicUsize::new(0)),
        creates: Arc::new(AtomicUsize::new(0)),
        truncate_create: Arc::new(AtomicBool::new(false)),
        reject: Arc::new(AtomicBool::new(false)),
    };
    let app = axum::Router::new()
        .route("/token", post(exchange))
        .route("/boxd.api.v1.BoxdApi/{method}", post(rpc))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = Boxd::new("fixture-api-key".into(), Some("fixture-org".into())).unwrap();
    provider.set_test_endpoint(format!("http://{address}"));
    let created = provider
        .create_from_snapshot("zuse-runtime-fixture", "w-fixture", true)
        .await
        .unwrap();
    assert_eq!(created.id, "vm-fixture");
    assert_eq!(created.state, MachineState::Running);
    assert!(created.memory_bytes.is_none());
    let (one, two) = tokio::join!(
        provider.inspect("vm-fixture"),
        provider.inspect("vm-fixture")
    );
    for machine in [one, two] {
        let machine = machine.unwrap().unwrap();
        assert_eq!(machine.vcpu, Some(4));
        assert_eq!(machine.memory_bytes, Some(17179869184));
    }
    assert_eq!(fixture.exchanges.load(Ordering::Relaxed), 1);
    assert!(provider.inspect("missing").await.unwrap().is_none());
    provider
        .set_auto_hibernate("vm-fixture", 600)
        .await
        .unwrap();
    provider.hibernate("vm-fixture").await.unwrap();
    provider.wake("vm-fixture").await.unwrap();
    provider.start("vm-fixture").await.unwrap();
    assert_eq!(provider.resume("vm-fixture").await.unwrap(), 123456);
    let output = provider
        .exec("vm-fixture", "printf cloud-ready")
        .await
        .unwrap();
    assert_eq!(output.stdout, b"cloud-ready");
    assert_eq!(output.stderr, b"diagnostic");
    assert_eq!(output.exit_code, 2);
    provider
        .upload(
            "vm-fixture",
            "/run/zuse-secrets/bootstrap",
            b"fixture-config",
        )
        .await
        .unwrap();
    provider.destroy("vm-fixture").await.unwrap();
    fixture.reject.store(true, Ordering::Relaxed);
    assert_eq!(
        provider.inspect("vm-fixture").await.unwrap_err(),
        ProviderError::Rejected
    );
    fixture.reject.store(false, Ordering::Relaxed);
    fixture.truncate_create.store(true, Ordering::Relaxed);
    assert_eq!(
        provider
            .create_from_snapshot("zuse-runtime-fixture", "w-fixture", true)
            .await
            .unwrap_err(),
        ProviderError::OutcomeUnknown
    );
    assert_eq!(
        fixture.creates.load(Ordering::Relaxed),
        2,
        "One normal allocation and one ambiguous allocation; never an automatic replay"
    );
    server.abort();
}
