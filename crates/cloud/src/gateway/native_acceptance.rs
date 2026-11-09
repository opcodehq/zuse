//! Isolated routing acceptance: real Room + Rust guest, fixture authorization, no database.
use super::*;
use axum::{Router, extract::ws::WebSocketUpgrade, http::HeaderMap, routing::get};
use zuse_protocol::cloud_gateway::{RUST_PROTOCOL, RUST_RPC};
use zuse_rpc::{RpcError, RpcReply, RpcService};

struct Service;
#[async_trait::async_trait]
impl RpcService for Service {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            "EngineInfo" => RpcReply::value(&json!({"deviceId":"native-cloud-guest"})),
            "Echo" => RpcReply::value(&params),
            "Watch" => Ok(RpcReply::Stream(Box::pin(
                futures::stream::once(async { json!({"ready":true}) })
                    .chain(futures::stream::pending()),
            ))),
            _ => Err(RpcError::Failed("Unknown fixture method".into())),
        }
    }
}

#[tokio::test]
#[ignore = "requires JDK21 and Android Gradle; isolated fixture authority, no live cloud account"]
async fn android_client_through_real_gateway_and_guest() {
    let room = Arc::new(SessionRoom(Arc::new(Room {
        workspace: "workspace".into(),
        token: "fixture-lease".into(),
        fence: (1, 1),
        lease_valid_until: Mutex::new(Instant::now() + Duration::from_secs(180)),
        state: Mutex::new(State {
            peers: HashMap::new(),
            runtime: None,
            closed: false,
        }),
        bytes: Arc::new(Semaphore::new(32 * 1024 * 1024)),
    })));
    let expires = now() + 120_000;
    let app = Router::new().route(
        "/v1/cloud/workspaces/workspace/gateway",
        get({
            let room = room.clone();
            move |headers: HeaderMap, upgrade: WebSocketUpgrade| {
                let room = room.clone();
                async move {
                    let protocols = headers
                        .get("sec-websocket-protocol")
                        .unwrap()
                        .to_str()
                        .unwrap();
                    assert!(protocols.contains(RUST_PROTOCOL));
                    assert!(
                        protocols.contains("runtime-fixture")
                            || protocols.contains("client-fixture")
                    );
                    let grant = Grant {
                        workspace: "workspace".into(),
                        generation: 1,
                        epoch: 1,
                        runtime: protocols.contains("runtime-fixture"),
                        protocol: RUST_PROTOCOL.into(),
                        actor: "owner".into(),
                        permission: "edit".into(),
                        expires_at: expires,
                    };
                    upgrade
                        .protocols([RUST_PROTOCOL])
                        .on_upgrade(move |socket| async move {
                            let permit = Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
                            room.serve(grant, socket, permit).await;
                        })
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!(
        "ws://{}/v1/cloud/workspaces/workspace/gateway",
        listener.local_addr().unwrap()
    );
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap()
    });
    let guest_stop = stop.clone();
    let grant = zuse_rpc::cloud::Grant {
        workspace_id: "workspace".into(),
        ws_url: address.clone(),
        protocol: RUST_PROTOCOL.into(),
        rpc_protocol: RUST_RPC.into(),
        credential: "runtime-fixture".into(),
        generation: 1,
        gateway_epoch: 1,
        expires_at: expires,
    };
    let guest = tokio::spawn(async move {
        zuse_rpc::cloud::serve_guest(Arc::new(Service), grant, "workspace", "owner", guest_stop)
            .await
    });
    let result = tokio::task::spawn_blocking(move || {
        let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::process::Command::new(checkout.join("apps/android/gradlew"))
            .args([
                "-p",
                "apps/android",
                ":core:test",
                "--tests",
                "sh.zuse.core.RealCloudTests",
                "--rerun-tasks",
                "--no-daemon",
            ])
            .current_dir(checkout)
            .env("ZUSE_NATIVE_CLOUD_ADDRESS", address)
            .env("ZUSE_NATIVE_CLOUD_EXPIRY", expires.to_string())
            .status()
    })
    .await
    .unwrap();
    stop.cancel();
    room.0.close_all(UNAVAILABLE);
    guest.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap().success());
}
