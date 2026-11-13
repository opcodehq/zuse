use super::*;
use crate::providers::boxd_wire as wire;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, Response},
    routing::post,
};
use prost::Message;
use std::{collections::HashMap, sync::Mutex};
#[derive(Default)]
struct Data {
    machines: HashMap<String, wire::MachineDetails>,
    snapshots: HashMap<String, wire::SnapshotInfo>,
    uploads: HashMap<(String, String), Vec<u8>>,
    stamps: HashMap<String, Value>,
    snapshot_stamps: HashMap<String, Value>,
    hygiene: HashMap<String, Value>,
    snapshot_hygiene: HashMap<String, Value>,
    stops: usize,
    initial_boots: HashMap<String, String>,
    creates: usize,
    captures: usize,
    setups: usize,
    deletes: usize,
    snapshot_deletes: usize,
    fault: String,
    corrupt: bool,
    revoke_during_load: bool,
    placement_revoked: bool,
    placement_checks: usize,
    expected: Value,
    preparation_plan: Option<PreparationPlan>,
    origin: String,
}
#[derive(Clone)]
struct Fixture(Arc<Mutex<Data>>);
fn frame(bytes: &[u8], trailer: bool) -> Vec<u8> {
    let mut out = vec![if trailer { 128 } else { 0 }];
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}
fn reply<M: Message>(value: &M) -> Response<Body> {
    let mut out = frame(&value.encode_to_vec(), false);
    out.extend(frame(b"grpc-status: 0\r\n", true));
    Response::builder()
        .header("content-type", "application/grpc-web+proto")
        .body(Body::from(out))
        .unwrap()
}
fn missing() -> Response<Body> {
    Response::builder()
        .header("content-type", "application/grpc-web+proto")
        .body(Body::from(frame(b"grpc-status: 5\r\n", true)))
        .unwrap()
}
fn lost() -> Response<Body> {
    Response::builder()
        .header("content-type", "application/grpc-web+proto")
        .body(Body::from(vec![0, 0, 0, 0, 255, 1]))
        .unwrap()
}
fn take_fault(data: &mut Data, name: &str) -> bool {
    if data.fault == name {
        data.fault.clear();
        true
    } else {
        false
    }
}
async fn exchange(Json(value): Json<Value>) -> Json<Value> {
    assert_eq!(value["api_key"], "fixture-project-cache-key");
    Json(
        json!({"token":"fixture-project-cache-session","expires_at":chrono::Utc::now().timestamp()+3600}),
    )
}
async fn rpc(
    State(f): State<Fixture>,
    Path(method): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    assert_eq!(
        headers["authorization"],
        "Bearer fixture-project-cache-session"
    );
    assert!(body.len() >= 5);
    let n = u32::from_be_bytes(body[1..5].try_into().unwrap()) as usize;
    let body = &body[5..5 + n];
    let mut data = f.0.lock().unwrap();
    match method.as_str() {
        "GetVm" => {
            let request = wire::MachineId::decode(body).unwrap();
            match data
                .machines
                .values()
                .find(|m| m.vm_id == request.vm_id || m.name == request.vm_id)
            {
                Some(m) => reply(m),
                None => missing(),
            }
        }
        "CreateVmFromSnapshot" => {
            let request = wire::CreateFromSnapshot::decode(body).unwrap();
            assert!(request.isolated);
            assert!(request.name.starts_with("pc-b-") || request.name.starts_with("pc-v-"));
            data.creates += 1;
            let id = format!("machine-{}", data.creates);
            let machine = wire::MachineDetails {
                vm_id: id.clone(),
                name: request.name.clone(),
                status: "running".into(),
                vcpu: 1,
                memory_bytes: 1024 * 1024 * 1024,
                source_info: Some(wire::VmSource {
                    kind: "snapshot".into(),
                    name: request.snapshot.clone(),
                    version: 1,
                    id: request.snapshot.clone(),
                }),
            };
            if let Some(stamp) = data.snapshot_stamps.get(&request.snapshot).cloned() {
                let mut stamp = stamp;
                if data.corrupt {
                    stamp["receipt"]["runtimeSha256"] = json!("b".repeat(64));
                }
                data.stamps.insert(id.clone(), stamp);
                let ready = data.snapshot_hygiene[&request.snapshot].clone();
                data.hygiene.insert(id.clone(), ready);
            }
            data.machines.insert(id.clone(), machine);
            if take_fault(&mut data, "create") {
                return lost();
            }
            reply(&wire::CreatedMachine {
                vm_id: id,
                name: request.name,
                status: "running".into(),
            })
        }
        "UploadFile" => {
            let request = wire::UploadFile::decode(body).unwrap();
            assert!(request.path.starts_with("/opt/zuse/preparation/"));
            let n = request.data.len();
            data.uploads
                .insert((request.vm_id, request.path), request.data);
            reply(&wire::Uploaded {
                bytes_written: n as u64,
            })
        }
        "Exec" => {
            let request = wire::ExecChunk::decode(body).unwrap();
            assert!(!request.command.contains("fixture-project-cache-key"));
            assert!(!request.command.contains("fixture-project-cache-session"));
            let output = if request.command.contains("managed-base-fresh-boot.py") {
                use base64::Engine;
                let encoded = request
                    .command
                    .rsplit(' ')
                    .next()
                    .unwrap()
                    .trim_matches('\'');
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .unwrap();
                let binding: crate::managed_bases::bootstrap::Request =
                    serde_json::from_slice(&bytes).unwrap();
                assert_eq!(binding.source_machine_id, request.vm_id);
                let digest = binding.digest().unwrap();
                if let Some(prior) = data
                    .initial_boots
                    .insert(request.vm_id.clone(), digest.clone())
                {
                    assert_eq!(prior, digest)
                }
                let mut value = serde_json::to_value(binding).unwrap();
                value["status"] = json!("ready");
                value["requestSha256"] = json!(digest);
                value["unitStartSubmitted"] = json!(true);
                value["resident"] = json!({"residentPid":42,"residentStartTicks":10,"bootNonceSha256":"a".repeat(64),"ipcVerified":true,"unbound":true,"childAlive":false,"recoveryAlive":false});
                if take_fault(&mut data, "bootstrap") {
                    return lost();
                }
                value
            } else if request.command.contains("project-cache-capabilities") {
                json!({"version":1,"runtimeSha256":"a".repeat(64),"environmentCacheVersion":1,"secretFreeSourceBundleVersion":1,"retainedTaskStorageAbsentRequired":true,"credentialFreePreparationRequired":true,"preparationCaptureVersion":1})
            } else if request.command.contains("capture-project-cache") {
                assert!(!request.command.contains("legacy-builder"));
                if take_fault(&mut data, "capture-proof-read") {
                    return lost();
                }
                if let Some(ready) = data.hygiene.get(&request.vm_id) {
                    ready.clone()
                } else if request.command.ends_with(" --status") {
                    json!({"state":"not-started"})
                } else if data.fault == "unsupported-quiesce" {
                    json!({"state":"failed"})
                } else {
                    data.stops += 1;
                    let expected = &data.expected["receipt"];
                    let ready = json!({"state":"ready","receipt":{"version":1,"preparationCaptureVersion":1,"cacheKey":expected["cacheKey"],"operationId":expected["operationId"],"runtimeSha256":expected["runtimeSha256"],"preparedReceiptSha256":"d".repeat(64),"hygiene":{"operationId":expected["operationId"],"runtimeSha256":expected["runtimeSha256"],"atomicStopAcknowledged":true,"legacyBuilder":false,"runtimeDataAbsent":true,"originalResidentExited":true,"supervisorLockReleased":true,"enabledBootServiceRetained":true}}});
                    data.hygiene.insert(request.vm_id.clone(), ready.clone());
                    if take_fault(&mut data, "quiesce") {
                        return lost();
                    }
                    ready
                }
            } else if request.command.starts_with("python3 -I -c") {
                json!({"receipt":data.stamps.get(&request.vm_id),"intent":false,"failed":false})
            } else if request.command.contains("prepare-project-cache --plan") {
                let plan: Value = serde_json::from_slice(
                    data.uploads
                        .get(&(
                            request.vm_id.clone(),
                            "/opt/zuse/preparation/project-cache.json".into(),
                        ))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    plan["sourceBundleSha256"],
                    data.expected["receipt"]["sourceBundleSha256"]
                );
                assert!(plan.get("agentConnectionId").is_none());
                assert!(plan.get("token").is_none());
                data.setups += 1;
                if take_fault(&mut data, "setup-failed") {
                    return reply(&wire::ExecChunk {
                        data: b"{}".to_vec(),
                        exit_code: 23,
                        ..Default::default()
                    });
                }
                let stamp = data.expected.clone();
                data.stamps.insert(request.vm_id.clone(), stamp);
                if take_fault(&mut data, "prepare") {
                    return lost();
                }
                json!({})
            } else {
                json!({})
            };
            reply(&wire::ExecChunk {
                data: serde_json::to_vec(&output).unwrap(),
                exit_code: 0,
                ..Default::default()
            })
        }
        "CreateSnapshot" => {
            let request = wire::CreateSnapshotRequest::decode(body).unwrap();
            assert!(request.name.starts_with("pc-s-"));
            assert_eq!(
                data.hygiene[&request.vm]["state"], "ready",
                "never snapshot before atomic clean capture barrier"
            );
            data.captures += 1;
            let id = format!("project-snapshot-{}", data.captures);
            let snapshot = wire::SnapshotInfo {
                snapshot_id: id.clone(),
                name: request.name.clone(),
                version: 1,
                status: "ready".into(),
                size_bytes: 100,
                updated_at: 0,
                vcpu: 1,
                memory_bytes: 1024 * 1024 * 1024,
                use_count: 0,
                created_at: 0,
            };
            let stamp = data.stamps[&request.vm].clone();
            data.snapshot_stamps.insert(id.clone(), stamp);
            let ready = data.hygiene[&request.vm].clone();
            data.snapshot_hygiene.insert(id.clone(), ready);
            data.snapshots.insert(id.clone(), snapshot);
            if take_fault(&mut data, "capture") {
                return lost();
            }
            reply(&wire::CreateSnapshotResponse {
                snapshot_id: id,
                name: request.name,
                version: 1,
                status: "ready".into(),
            })
        }
        "GetSnapshot" => {
            let request = wire::SnapshotRequest::decode(body).unwrap();
            if request.name == "base-artifact" {
                return reply(&wire::SnapshotResponse {
                    snapshot: Some(wire::SnapshotInfo {
                        snapshot_id: "base-artifact".into(),
                        name: "base-artifact".into(),
                        version: 1,
                        status: "ready".into(),
                        ..Default::default()
                    }),
                });
            }
            let snapshot = data
                .snapshots
                .values()
                .find(|s| s.snapshot_id == request.name || s.name == request.name)
                .cloned();
            reply(&wire::SnapshotResponse { snapshot })
        }
        "DestroyVm" => {
            let request = wire::MachineId::decode(body).unwrap();
            assert!(
                data.machines
                    .get(&request.vm_id)
                    .is_some_and(|m| m.name.starts_with("pc-b-")
                        || m.name.starts_with("pc-v-")
                        || (m.vm_id == "other-slot" && m.name == "held-task"))
            );
            data.deletes += 1;
            data.machines.remove(&request.vm_id);
            if take_fault(&mut data, "destroy") {
                return lost();
            }
            reply(&wire::Empty {})
        }
        "DeleteSnapshot" => {
            let request = wire::SnapshotRequest::decode(body).unwrap();
            assert_ne!(request.name, "base-artifact");
            data.snapshot_deletes += 1;
            data.snapshots.remove(&request.name);
            reply(&wire::Empty {})
        }
        other => panic!("unexpected provider method {other}"),
    }
}
struct Source(Arc<Mutex<Data>>);
#[async_trait]
impl Bundles for Source {
    async fn authorize_placement(&self, request: &Request) -> Result<(), ApiError> {
        assert_eq!(request.actor_id, "owner");
        let mut data = self.0.lock().unwrap();
        data.placement_checks += 1;
        if data.placement_revoked {
            return Err(ApiError::forbidden("fixture_source_authorization_revoked"));
        }
        Ok(())
    }

    async fn resolve(&self, request: &Request) -> Result<super::super::source::Bundle, ApiError> {
        let data = self.0.lock().unwrap();
        let plan = data.preparation_plan.as_ref().unwrap();
        assert_eq!(request.owner, plan.identity.owner_id);
        assert_eq!(request.project, plan.identity.project_id);
        Ok(super::super::source::Bundle {
            commit: plan.identity.source_commit.clone(),
            sha256: format!("{:x}", Sha256::digest(b"public-source-fixture")),
            bytes: b"public-source-fixture".to_vec(),
            dependency_inputs: plan.identity.dependency_inputs.clone(),
            recipe: plan.recipe.clone(),
            recipe_source: plan.identity.recipe_source.clone(),
        })
    }
    async fn load(&self, sha: &str, owner: &str, project: &str) -> Result<Vec<u8>, ApiError> {
        assert_eq!(owner, "owner");
        assert_eq!(project, "project");
        {
            let mut data = self.0.lock().unwrap();
            if data.revoke_during_load {
                data.placement_revoked = true;
            }
        }
        let bytes = b"public-source-fixture".to_vec();
        assert_eq!(sha, format!("{:x}", Sha256::digest(&bytes)));
        Ok(bytes)
    }
}
fn operation() -> Operation {
    let recipe = zuse_protocol::cloud_computers::EnvironmentRecipe::default();
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&recipe).unwrap()));
    Operation {
        operation_id: format!("cacheop_{}", uuid::Uuid::new_v4().simple()),
        request: Request {
            request_id: "request".into(),
            owner: "owner".into(),
            actor_id: "owner".into(),
            actor: Value::Null,
            project: "project".into(),
            environment_revision: 1,
            requested_ref: "main".into(),
            repository_identity: "github.com/example/project".into(),
        },
        plan: PreparationPlan {
            identity: CacheIdentity {
                version: 1,
                owner_id: "owner".into(),
                project_id: "project".into(),
                repository_identity: "github.com/example/project".into(),
                provider: zuse_protocol::cloud_computers::ProviderId::Boxd,
                architecture: "x86_64".into(),
                base_release_id: "release".into(),
                base_artifact_id: "base-artifact".into(),
                base_artifact_version: Some("1".into()),
                base_runtime_sha256: "a".repeat(64),
                recipe_source: "project".into(),
                recipe_digest: digest,
                source_commit: "c".repeat(40),
                dependency_inputs: Default::default(),
                staging_pilot_sha256: None,
            },
            recipe,
            base_artifact_binding: None,
            initial_runtime_bootstrap: Some(
                crate::managed_bases::bootstrap::InitialRuntimeBootstrap {
                    version: 1,
                    mode: "stopped-memory-snapshot-v1".into(),
                    runtime_sha256: "a".repeat(64),
                    supervisor_sha256: "b".repeat(64),
                    service_sha256: "c".repeat(64),
                    manifest_sha256: "d".repeat(64),
                    helper_sha256: "e".repeat(64),
                    runtime_update_helper_sha256: None,
                },
            ),
            source_bundle_sha256: format!("{:x}", Sha256::digest(b"public-source-fixture")),
        },
        phase: "allocate".into(),
        resources: json!({"effects":{},"provider":{}}),
    }
}
async fn fixture(
    op: &Operation,
    fault: &str,
) -> (BoxdPreparation, Fixture, tokio::task::JoinHandle<()>) {
    let f = Fixture(Arc::new(Mutex::new(Data {
        fault: fault.into(),
        expected: json!({"version":1,"identity":op.plan.identity,"receipt":expected(op).unwrap()}),
        preparation_plan: Some(op.plan.clone()),
        ..Default::default()
    })));
    let app = Router::new()
        .route(
            "/repos/example/project",
            axum::routing::get(|| async {
                Json(json!({"id":41,"private":false,"full_name":"example/project"}))
            }),
        )
        .route(
            "/repos/example/project/commits/{reference}",
            axum::routing::get(|| async { Json(json!({"sha":"c".repeat(40)})) }),
        )
        .route(
            "/repos/example/project/contents/.zuse/setup.json",
            axum::routing::get(|| async { axum::http::StatusCode::NOT_FOUND }),
        )
        .route("/token", post(exchange))
        .route("/boxd.api.v1.BoxdApi/{method}", post(rpc))
        .with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    f.0.lock().unwrap().origin = format!("http://{addr}");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = Boxd::new(
        "fixture-project-cache-key".into(),
        Some("fixture-org".into()),
    )
    .unwrap();
    provider.set_test_endpoint(format!("http://{addr}"));
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .unwrap();
    (
        BoxdPreparation::new(pool, Arc::new(provider), Arc::new(Source(f.0.clone()))),
        f,
        server,
    )
}
async fn tick(
    backend: &BoxdPreparation,
    op: &mut Operation,
) -> Result<Option<StepResult>, ApiError> {
    let phase = op.phase.clone();
    let mode = if op.resources["effects"][&phase]["requested"] == true {
        StepMode::Reconcile
    } else {
        StepMode::Begin
    };
    op.resources["effects"][&phase] = json!({"requested":true});
    let result = backend.step(op, mode).await?;
    match result {
        StepResult::Advance { phase, resources } => {
            op.phase = phase;
            op.resources["provider"] = resources;
            Ok(None)
        }
        StepResult::Pending(resources) => {
            op.resources["provider"] = resources;
            Ok(None)
        }
        value => Ok(Some(value)),
    }
}
#[tokio::test]
async fn real_boxd_transport_reconciles_original_create_prepare_capture_and_delete() {
    for fault in [
        "",
        "create",
        "bootstrap",
        "prepare",
        "quiesce",
        "capture",
        "destroy",
    ] {
        let mut op = operation();
        let (backend, f, server) = fixture(&op, fault).await;
        let mut ready = false;
        for _ in 0..30 {
            match tick(&backend, &mut op).await {
                Ok(Some(StepResult::Ready {
                    artifact_id,
                    receipt,
                    resources,
                    ..
                })) => {
                    assert_eq!(artifact_id, "project-snapshot-1");
                    receipt
                        .validate(&op.plan.identity, &op.operation_id)
                        .unwrap();
                    assert_eq!(resources["builderAbsent"], true);
                    assert_eq!(resources["verifierAbsent"], true);
                    ready = true;
                    break;
                }
                Ok(None) => {}
                Err(_) => {
                    assert!(!fault.is_empty());
                    assert!(f.0.lock().unwrap().fault.is_empty());
                }
                _ => panic!("unexpected outcome"),
            }
        }
        assert!(ready, "original operation never completed: {fault}");
        let data = f.0.lock().unwrap();
        assert_eq!(data.creates, 2);
        assert_eq!(data.captures, 1);
        assert_eq!(data.setups, 1);
        assert_eq!(
            data.initial_boots.len(),
            2,
            "original builder and new verifier both use shared initial bootstrap"
        );
        assert_eq!(
            data.stops, 1,
            "lost stop reply never reruns original capture barrier"
        );
        assert_eq!(data.deletes, 2);
        assert_eq!(data.snapshot_deletes, 0);
        assert!(data.machines.is_empty());
        drop(data);
        server.abort();
    }
}
#[tokio::test]
async fn mismatched_cache_readback_cleans_only_original_resources() {
    let mut op = operation();
    let (backend, f, server) = fixture(&op, "").await;
    while op.phase != "verify" {
        assert!(tick(&backend, &mut op).await.unwrap().is_none());
    }
    f.0.lock().unwrap().corrupt = true;
    assert_eq!(
        tick(&backend, &mut op).await.err().unwrap().code,
        "project_cache_snapshot_readback_mismatch"
    );
    op.phase = "cleanup".into();
    let mut cleaned = false;
    for _ in 0..12 {
        match tick(&backend, &mut op).await.unwrap() {
            Some(StepResult::Ready { .. }) => panic!("bad readback published"),
            Some(_) => panic!("unexpected terminal"),
            None if op.phase == "complete" => {
                cleaned = true;
                break;
            }
            None => {}
        }
    }
    assert!(cleaned);
    let data = f.0.lock().unwrap();
    assert!(data.machines.is_empty());
    assert!(data.snapshots.is_empty());
    assert_eq!(data.snapshot_deletes, 1);
    drop(data);
    server.abort();
}

#[tokio::test]
async fn unsupported_or_missing_capture_barrier_never_snapshots_and_cleans_original_builder() {
    for fault in ["unsupported", "missing", "unreadable"] {
        let unsupported = fault == "unsupported";
        let mut op = operation();
        let (backend, f, server) = fixture(
            &op,
            if unsupported {
                "unsupported-quiesce"
            } else {
                ""
            },
        )
        .await;
        while op.phase != if unsupported { "quiesce" } else { "capture" } {
            assert!(tick(&backend, &mut op).await.unwrap().is_none());
        }
        if fault == "missing" {
            f.0.lock().unwrap().hygiene.clear();
        } else if fault == "unreadable" {
            f.0.lock().unwrap().fault = "capture-proof-read".into();
        }
        let resources = match tick(&backend, &mut op).await.unwrap().unwrap() {
            StepResult::Failed { resources, .. } => resources,
            _ => panic!("must fail closed"),
        };
        op.phase = "cleanup".into();
        op.resources["provider"] = resources;
        for _ in 0..8 {
            if op.phase == "complete" {
                break;
            }
            assert!(tick(&backend, &mut op).await.unwrap().is_none());
        }
        let data = f.0.lock().unwrap();
        assert_eq!(data.captures, 0);
        assert_eq!(data.creates, 1);
        assert_eq!(data.deletes, 1);
        assert!(data.machines.is_empty());
        drop(data);
        server.abort();
    }
}

#[tokio::test]
async fn initial_boot_source_wire_rejects_changed_snapshot_version_kind_or_name_before_helper() {
    use crate::managed_bases::bootstrap::{Purpose, Request as BootRequest};
    use crate::runtime::allocation::AllocationProvider;
    for mode in ["ready", "kind", "source", "version", "name"] {
        let op = operation();
        let (backend, f, server) = fixture(&op, "").await;
        let name = names(&op).0;
        let machine = backend
            .provider
            .create_from_snapshot("base-artifact", &name, true)
            .await
            .unwrap();
        {
            let mut data = f.0.lock().unwrap();
            let source = data
                .machines
                .get_mut(&machine.id)
                .unwrap()
                .source_info
                .as_mut()
                .unwrap();
            match mode {
                "kind" => source.kind = "template".into(),
                "source" => source.id = "different".into(),
                "version" => source.version = 2,
                _ => {}
            }
        }
        let request = BootRequest {
            version: 1,
            provider: zuse_protocol::cloud_computers::ProviderId::Boxd,
            purpose: Purpose::InitialAllocation,
            operation_id: op.operation_id.clone(),
            workspace_id: "ws-original".into(),
            generation: 1,
            storage_incarnation_id: "disk-original".into(),
            source_machine_id: machine.id.clone(),
            image_pins: op.plan.initial_runtime_bootstrap.as_ref().unwrap().pins(),
        };
        let provider = crate::runtime::provider::RuntimeProvider::new(
            backend.provider.clone(),
            "default".into(),
        );
        let result = provider
            .bootstrap_initial_runtime(
                &machine.id,
                "base-artifact",
                if mode == "name" { "wrong-name" } else { &name },
                &request,
            )
            .await;
        let data = f.0.lock().unwrap();
        assert!(
            data.uploads.is_empty(),
            "no credential upload in fresh boot transport"
        );
        if mode == "ready" {
            assert!(result.unwrap().is_some());
            assert_eq!(data.initial_boots.len(), 1)
        } else {
            assert!(result.is_err());
            assert!(
                data.initial_boots.is_empty(),
                "source check must precede helper execution"
            )
        }
        drop(data);
        server.abort();
    }
}

#[tokio::test]
async fn revocation_during_object_loading_prevents_source_placement_and_cleans_original_builder() {
    let mut op = operation();
    let (backend, fixture, server) = fixture(&op, "").await;
    while op.phase != "prepare" {
        assert!(tick(&backend, &mut op).await.unwrap().is_none());
    }
    fixture.0.lock().unwrap().revoke_during_load = true;
    assert_eq!(
        tick(&backend, &mut op).await.err().unwrap().code,
        "fixture_source_authorization_revoked"
    );
    {
        let data = fixture.0.lock().unwrap();
        assert!(data.placement_checks > 0);
        assert!(
            data.uploads
                .keys()
                .all(|(_, path)| !path.starts_with("/opt/zuse/preparation/source"))
        );
        assert_eq!(data.setups, 0);
        assert_eq!(data.captures, 0);
    }
    op.phase = "cleanup".into();
    for _ in 0..12 {
        if op.phase == "complete" {
            break;
        }
        tick(&backend, &mut op).await.unwrap();
    }
    assert_eq!(op.phase, "complete");
    assert!(fixture.0.lock().unwrap().machines.is_empty());
    assert_eq!(fixture.0.lock().unwrap().snapshot_deletes, 0);
    server.abort();
}

/// Full real-SQL controller + actual Boxd wire fixture. Runs in a separately
/// spawned process whose operator pilot environment is immutable for its life.
pub(in crate::project_caches) async fn exercise_staging_pilot(pool: &PgPool) {
    use super::super::worker::Authorization;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use std::sync::atomic::{AtomicI64, Ordering};
    use zuse_protocol::cloud_computers::{EnvironmentRecipe, SaveEnvironment};
    let now = chrono::Utc::now().timestamp_millis();
    let authority = super::super::pilot::current(now).unwrap().unwrap();
    let mut prototype = operation();
    prototype.plan.identity.staging_pilot_sha256 = Some(authority.pilot.fingerprint().unwrap());
    let (mut backend, f, server) = fixture(&prototype, "").await;
    backend.pool = pool.clone();
    let backend = Arc::new(backend);
    let signing = ed25519_dalek::SigningKey::from_bytes(&[81; 32]);
    let jwk=json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([81;32]),"x":B64.encode(signing.verifying_key().as_bytes())}).to_string();
    let runtime = Arc::new(
        crate::runtime::RuntimeControl::from_pool(
            pool.clone(),
            crate::crypto::MasterKey::parse(&B64.encode([82; 32])).unwrap(),
            crate::tickets::TicketSigner::from_jwk(&jwk, "https://fixture.invalid").unwrap(),
        )
        .with_creation_accounts(vec!["owner".into()])
        .with_staging_base_pin(Some(authority.pin.clone())),
    );
    struct ApiAuthority {
        runtime: Arc<crate::runtime::RuntimeControl>,
        jwk: String,
        clock: Arc<AtomicI64>,
    }
    #[async_trait]
    impl Authorization for ApiAuthority {
        async fn authorize(&self, request: &Request) -> Result<(), ApiError> {
            let now = self.clock.load(Ordering::SeqCst);
            let signer =
                crate::tickets::TicketSigner::from_jwk(&self.jwk, "https://fixture.invalid")?;
            let proof = signer.allocation_scope_proof(
                &format!("preparation:{}", request.request_id),
                &format!("project:{}", request.project),
                request.environment_revision,
                now,
            )?;
            self.runtime
                .authorize_project_preparation(&request.project, &request.request_id, &proof, now)
                .await
        }
    }
    let clock = Arc::new(AtomicI64::new(now));
    runtime
        .project_cache_control
        .install(
            Arc::new(ApiAuthority {
                runtime: runtime.clone(),
                jwk,
                clock: clock.clone(),
            }),
            backend.clone(),
        )
        .unwrap();
    let setup = crate::computers::ComputerControl::from_pool(pool.clone(), vec!["owner".into()]);
    let protected: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    async fn save(
        setup: &crate::computers::ComputerControl,
        pool: &PgPool,
        revision: i64,
        recipe: EnvironmentRecipe,
    ) -> String {
        setup
            .save_environment(
                "owner",
                "project",
                SaveEnvironment {
                    expected_revision: revision,
                    recipe,
                },
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        sqlx::query_scalar("SELECT request_id FROM zuse_rust_project_preparation_requests WHERE owner_id='owner' AND project_id='project' AND environment_revision=$1 ORDER BY created_at LIMIT 1").bind(revision+1).fetch_one(pool).await.unwrap()
    }
    async fn tick(
        pool: &PgPool,
        runtime: &crate::runtime::RuntimeControl,
        f: &Fixture,
        clock: &AtomicI64,
        request: &str,
        now: i64,
    ) -> bool {
        sqlx::query("UPDATE zuse_rust_project_preparation_requests SET next_attempt_at=0 WHERE request_id=$1").bind(request).execute(pool).await.unwrap();
        sqlx::query("UPDATE zuse_rust_project_cache_operations SET next_attempt_at=0 WHERE owner_id='owner' AND project_id='project' AND state IN('queued','running','uncertain','cleaning')").execute(pool).await.unwrap();
        let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_project_cache_operations o WHERE owner_id='owner' AND project_id='project' AND state IN('queued','running','uncertain','cleaning') ORDER BY next_attempt_at,operation_id LIMIT 1").fetch_optional(pool).await.unwrap();
        if let Some(row) = row {
            let plan: PreparationPlan = serde_json::from_value(row["plan"].clone()).unwrap();
            let mut op = operation();
            op.operation_id = row["operation_id"].as_str().unwrap().into();
            op.plan = plan;
            f.0.lock().unwrap().expected =
                json!({"version":1,"identity":op.plan.identity,"receipt":expected(&op).unwrap()});
        }
        clock.store(now, Ordering::SeqCst);
        runtime.work_project_preparation_once(now).await.unwrap()
    }
    async fn drive(
        pool: &PgPool,
        runtime: &crate::runtime::RuntimeControl,
        f: &Fixture,
        clock: &AtomicI64,
        request: &str,
    ) -> String {
        for _ in 0..70 {
            tick(
                pool,
                runtime,
                f,
                clock,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await;
            let state: String = sqlx::query_scalar(
                "SELECT state FROM zuse_rust_project_preparation_requests WHERE request_id=$1",
            )
            .bind(request)
            .fetch_one(pool)
            .await
            .unwrap();
            if matches!(state.as_str(), "ready" | "failed") {
                return state;
            }
        }
        let rows: Vec<Value> =
            sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_project_cache_operations o")
                .fetch_all(pool)
                .await
                .unwrap();
        panic!("pilot did not reach terminal state: {rows:?}")
    }
    sqlx::raw_sql("CREATE TABLE zuse_rust_allocations(operation_id text PRIMARY KEY,workspace_id text,generation bigint,snapshot_id text,machine_id text,phase text); CREATE TABLE zuse_rust_jobs(job_id text,resource_id text,resource_generation bigint,lease_token text,state text,lease_expires_at bigint); CREATE TABLE zuse_rust_control_owners(resource_id text,resource_kind text,backend text,generation bigint);").execute(pool).await.unwrap();
    async fn resolve_task(
        pool: &PgPool,
        runtime: &crate::runtime::RuntimeControl,
        f: &Fixture,
        name: &str,
        revision: i64,
        recipe: &EnvironmentRecipe,
        authority: &super::super::pilot::Authority,
        keep: bool,
    ) -> (Value, Value) {
        let config = json!({"runtimeGeneration":1,"rustActorId":"owner","rustAllocationOperationId":name,"managedReleaseId":"release","managedCandidateSha256":authority.pin.candidate_sha256,"managedCandidateArtifactId":"base-artifact","stagingProjectCachePilotSha256":authority.pilot.fingerprint().unwrap(),"environmentRecipe":recipe,"environmentRevision":revision,"rustRepositoryCheckout":{"baseRef":"main","fetchRemote":true}});
        sqlx::query("INSERT INTO api_cloud_workspaces SELECT (jsonb_populate_record(NULL::api_cloud_workspaces,to_jsonb(w)||jsonb_build_object('workspace_id',$1::text,'account_id','owner','project_id','project','provider_sandbox_id',NULL,'state','provisioning','desired_state','ready','request_config',$2::jsonb))).* FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'").bind(name).bind(config).execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO zuse_rust_allocations VALUES($1,$1,1,'base-artifact',NULL,'planned')",
        )
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO zuse_rust_jobs VALUES($1,$1,1,'pilot-lease','running',9999999999999)",
        )
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES($1,'workspace','rust',1)")
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
        let mut workspace: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
        let mut operation: Value = sqlx::query_scalar(
            "SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1",
        )
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
        let job = crate::jobs::Job {
            id: name.into(),
            kind: "workspace-allocate".into(),
            resource_id: name.into(),
            generation: 1,
            lease_token: "pilot-lease".into(),
            attempts: 1,
            payload: json!({}),
        };
        let origin = f.0.lock().unwrap().origin.clone();
        runtime
            .resolve_project_allocation_source_at(&job, &mut workspace, &mut operation, &origin)
            .await
            .unwrap();
        if !keep {
            sqlx::query("DELETE FROM api_cloud_workspaces WHERE workspace_id=$1")
                .bind(name)
                .execute(pool)
                .await
                .unwrap();
        }
        (workspace, operation)
    }
    let good = EnvironmentRecipe::default();
    let first = save(&setup, pool, 0, good.clone()).await;
    let (miss, miss_op) =
        resolve_task(pool, &runtime, &f, "pilot-miss", 1, &good, &authority, true).await;
    assert_eq!(miss_op["snapshot_id"], "base-artifact");
    assert!(miss["request_config"]["rustProjectCache"].is_null());
    assert_eq!(
        miss["request_config"]["rustRepositoryCheckout"]["fetchRemote"],
        true
    );
    // The same uncached task now holds the sole owned provider slot.
    sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id='other-slot' WHERE workspace_id='pilot-miss'").execute(pool).await.unwrap();
    f.0.lock().unwrap().machines.insert(
        "other-slot".into(),
        wire::MachineDetails {
            vm_id: "other-slot".into(),
            name: "held-task".into(),
            status: "running".into(),
            vcpu: 1,
            memory_bytes: 1024 * 1024 * 1024,
            source_info: None,
        },
    );
    for _ in 0..4 {
        tick(
            pool,
            &runtime,
            &f,
            &clock,
            &first,
            chrono::Utc::now().timestamp_millis(),
        )
        .await;
        let code:Option<String>=sqlx::query_scalar("SELECT last_error_code FROM zuse_rust_project_preparation_requests WHERE request_id=$1").bind(&first).fetch_one(pool).await.unwrap();
        if code.as_deref() == Some("staging_project_cache_resource_limit") {
            break;
        }
    }
    assert_eq!(
        f.0.lock().unwrap().creates,
        0,
        "budget deferral must precede provider allocation"
    );
    assert_eq!(f.0.lock().unwrap().captures, 0);
    let deferred: String = sqlx::query_scalar(
        "SELECT operation_id FROM zuse_rust_project_cache_operations WHERE owner_id='owner' AND project_id='project' AND state='queued' LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let submitted:bool=sqlx::query_scalar("SELECT coalesce(resource_receipts#>>'{effects,allocate,requested}','false')='true' FROM zuse_rust_project_cache_operations WHERE operation_id=$1").bind(&deferred).fetch_one(pool).await.unwrap();
    assert!(
        !submitted,
        "deferred operation must not claim a provider request"
    );
    backend.provider.destroy("other-slot").await.unwrap();
    assert!(
        backend
            .provider
            .inspect("other-slot")
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("DELETE FROM api_cloud_workspaces WHERE workspace_id='pilot-miss'")
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(drive(pool, &runtime, &f, &clock, &first).await, "ready");
    let completed: String = sqlx::query_scalar(
        "SELECT preparation_operation_id FROM zuse_rust_project_cache_entries WHERE state='ready'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        completed, deferred,
        "capacity recovery must continue the original eligible operation"
    );
    let value: Value = sqlx::query_scalar(
        "SELECT identity FROM zuse_rust_project_cache_entries WHERE state='ready'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let identity: CacheIdentity = serde_json::from_value(value).unwrap();
    let key = identity.key().unwrap();
    let mut tx = pool.begin().await.unwrap();
    let hit = lookup_tx(&mut tx, &identity).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(hit.artifact_id, "project-snapshot-1");
    let (task_hit, task_hit_op) =
        resolve_task(pool, &runtime, &f, "pilot-hit", 1, &good, &authority, false).await;
    assert_eq!(task_hit_op["snapshot_id"], hit.artifact_id);
    assert_eq!(
        task_hit["request_config"]["rustProjectCache"]["cacheKey"],
        key
    );
    assert_eq!(
        task_hit["request_config"]["rustRepositoryCheckout"]["fetchRemote"],
        false
    );

    assert_eq!(
        f.0.lock().unwrap().creates,
        2,
        "one builder and one verifier, serially"
    );
    assert!(f.0.lock().unwrap().machines.is_empty());
    let original_receipt: Value = sqlx::query_scalar(
        "SELECT receipt FROM zuse_rust_project_cache_entries WHERE cache_key=$1",
    )
    .bind(&key)
    .fetch_one(pool)
    .await
    .unwrap();
    let mut plain = identity.clone();
    plain.staging_pilot_sha256 = None;
    let mut tx = pool.begin().await.unwrap();
    assert!(lookup_tx(&mut tx, &plain).await.unwrap().is_none());
    tx.rollback().await.unwrap();
    let mut other = identity.clone();
    other.staging_pilot_sha256 = Some("f".repeat(64));
    let mut tx = pool.begin().await.unwrap();
    assert!(lookup_tx(&mut tx, &other).await.unwrap().is_none());
    tx.rollback().await.unwrap();
    let mut failing = good.clone();
    failing.scripts.initialize = Some("exit 23".into());
    {
        let mut data = f.0.lock().unwrap();
        let plan = data.preparation_plan.as_mut().unwrap();
        plan.recipe = failing.clone();
        plan.identity.recipe_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&failing).unwrap())
        );
        data.fault = "setup-failed".into();
    }
    let failed = save(&setup, pool, 1, failing.clone()).await;
    assert_eq!(drive(pool, &runtime, &f, &clock, &failed).await, "failed");
    assert!(f.0.lock().unwrap().machines.is_empty());
    assert_eq!(
        f.0.lock().unwrap().captures,
        1,
        "failed initialization never snapshots a computer"
    );
    assert_eq!(sqlx::query_scalar::<_,Value>("SELECT receipt FROM zuse_rust_project_cache_entries WHERE cache_key=$1 AND state='ready'").bind(&key).fetch_one(pool).await.unwrap(),original_receipt);
    {
        let mut data = f.0.lock().unwrap();
        let plan = data.preparation_plan.as_mut().unwrap();
        plan.recipe = good.clone();
        plan.identity.recipe_digest = identity.recipe_digest.clone();
    }
    let restored = save(&setup, pool, 2, good.clone()).await;
    assert_eq!(drive(pool, &runtime, &f, &clock, &restored).await, "ready");
    let mut tx = pool.begin().await.unwrap();
    let restored_hit = lookup_tx(&mut tx, &identity).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    assert_eq!(restored_hit.cache_key, key);
    assert_eq!(restored_hit.artifact_id, hit.artifact_id);
    let (after_failure, after_failure_op) = resolve_task(
        pool,
        &runtime,
        &f,
        "pilot-restored-hit",
        3,
        &good,
        &authority,
        false,
    )
    .await;
    assert_eq!(after_failure_op["snapshot_id"], hit.artifact_id);
    assert_eq!(
        after_failure["request_config"]["rustProjectCache"]["cacheKey"],
        key
    );

    // Original create starts, then its reply is lost. Expiry must reconcile that
    // exact name/resource and delete it, never allocate a replacement or capture.
    {
        let mut data = f.0.lock().unwrap();
        let plan = data.preparation_plan.as_mut().unwrap();
        plan.recipe = failing.clone();
        plan.identity.recipe_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&failing).unwrap())
        );
        data.fault = "create".into();
    }
    let lost = save(&setup, pool, 3, failing).await;
    for _ in 0..8 {
        tick(
            pool,
            &runtime,
            &f,
            &clock,
            &lost,
            chrono::Utc::now().timestamp_millis(),
        )
        .await;
        if !f.0.lock().unwrap().machines.is_empty() {
            break;
        }
    }
    assert_eq!(f.0.lock().unwrap().machines.len(), 1);
    let creates = f.0.lock().unwrap().creates;
    let original_ids: Vec<String> = f.0.lock().unwrap().machines.keys().cloned().collect();
    let mut tx = pool.begin().await.unwrap();
    assert!(
        super::super::pilot::capacity_tx(&mut tx, None)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let expired = authority.pilot.expires_at + 1;
    for _ in 0..8 {
        tick(pool, &runtime, &f, &clock, &lost, expired).await;
        if f.0.lock().unwrap().machines.is_empty() {
            break;
        }
    }
    assert!(
        f.0.lock().unwrap().machines.is_empty(),
        "original uncertain builder must be cleaned after expiry"
    );
    assert_eq!(
        f.0.lock().unwrap().creates,
        creates,
        "expiry cannot authorize a replacement"
    );
    assert_eq!(
        f.0.lock().unwrap().captures,
        1,
        "expiry cannot authorize capture/publication"
    );
    let rows:Vec<String>=sqlx::query_scalar("SELECT resource_id FROM zuse_rust_project_cache_resources WHERE resource_kind='builder' AND absent_observed_at IS NOT NULL").fetch_all(pool).await.unwrap();
    assert!(original_ids.iter().all(|id| rows.contains(id)));
    let mut tx = pool.begin().await.unwrap();
    assert!(
        !super::super::pilot::authorized_tx(&mut tx, &identity, expired)
            .await
            .unwrap_or(false)
    );
    tx.rollback().await.unwrap();
    // Real registry revocation also denies adoption; cleanup remains permitted.
    sqlx::query("UPDATE cloud_managed_bases SET state='retired' WHERE release_id='release'")
        .execute(pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(lookup_tx(&mut tx, &identity).await.unwrap().is_none());
    tx.rollback().await.unwrap();
    for _ in 0..8 {
        tick(
            pool,
            &runtime,
            &f,
            &clock,
            &lost,
            chrono::Utc::now().timestamp_millis(),
        )
        .await;
        if f.0.lock().unwrap().snapshots.is_empty() {
            break;
        }
    }
    assert!(f.0.lock().unwrap().snapshots.is_empty());
    assert_eq!(f.0.lock().unwrap().creates, creates);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM cloud_project_cache_capabilities")
            .fetch_one(pool)
            .await
            .unwrap(),
        0,
        "pilot never certifies a capability"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'"
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        protected,
        "original row/ciphertext must remain byte-equivalent"
    );
    server.abort();
}
