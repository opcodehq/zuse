//! Real localhost Boat HTTP + isolated PostgreSQL through the allocation worker.
use super::*;
use crate::{
    crypto::MasterKey,
    jobs,
    runtime::{RuntimeControl, provider::RuntimeProvider},
    tickets::TicketSigner,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct FailedBoot {
    pool: PgPool,
    workspace: String,
    mode: &'static str,
    create: Arc<Mutex<Option<Value>>>,
    before_error_config: Arc<Mutex<Option<Value>>>,
    create_count: Arc<AtomicUsize>,
    upload_count: Arc<AtomicUsize>,
}
fn sandbox(s: &FailedBoot) -> Value {
    let create = s.create.lock().unwrap();
    let body = create.as_ref().unwrap();
    let labels = match s.mode {
        "missing-labels"
        | "name-only"
        | "missing-receipt"
        | "forged-receipt"
        | "stale-receipt"
        | "source-receipt"
        | "incarnation-receipt" => json!({}),
        "conflicting-labels" => {
            json!({"zuse-source-artifact":"different-source","zuse-allocation-name":body["name"]})
        }
        "conflicting-operation" => {
            json!({"zuse-source-artifact":body["from"],"zuse-allocation-name":body["name"],"zuse-allocation-operation":"other-operation"})
        }
        "nonobject-labels" => Value::Null,
        _ => body["labels"].clone(),
    };
    json!({"id":"bx_23456789","name":body["name"],"state":match s.mode {"readiness-starting"=>"init","readiness-stopped"=>"stopped","readiness-unknown"=>"unexpected-state",_=>"idle"},"hydrated":!s.mode.starts_with("readiness-"),"labels":labels})
}
async fn list(axum::extract::State(s): axum::extract::State<FailedBoot>) -> Json<Value> {
    if s.mode == "name-only" {
        let name: String = sqlx::query_scalar(
            "SELECT machine_name FROM zuse_rust_allocations WHERE workspace_id=$1",
        )
        .bind(&s.workspace)
        .fetch_one(&s.pool)
        .await
        .unwrap();
        *s.create.lock().unwrap() = Some(json!({"name":name,"labels":{}}));
        Json(json!({"sandboxes":[sandbox(&s)],"pageInfo":{"nextCursor":null}}))
    } else {
        Json(json!({"sandboxes":[],"pageInfo":{"nextCursor":null}}))
    }
}
async fn create(
    axum::extract::State(s): axum::extract::State<FailedBoot>,
    Json(body): Json<Value>,
) -> Json<Value> {
    s.create_count.fetch_add(1, Ordering::SeqCst);
    assert_eq!(body["from"], "managed-artifact");
    *s.create.lock().unwrap() = Some(body);
    Json(json!({"sandbox":sandbox(&s)}))
}
async fn get(
    axum::extract::State(s): axum::extract::State<FailedBoot>,
) -> axum::response::Response {
    if s.mode.starts_with("readiness-") {
        let polls: i64 = sqlx::query_scalar("SELECT COALESCE((payload->>'readinessPollCount')::bigint,0) FROM zuse_rust_jobs WHERE resource_id=$1")
            .bind(&s.workspace).fetch_one(&s.pool).await.unwrap();
        if polls >= 4 && matches!(s.mode, "readiness-503" | "readiness-429") {
            return (
                if s.mode == "readiness-429" {
                    StatusCode::TOO_MANY_REQUESTS
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                Json(json!({"error":"private-provider-body-token"})),
            )
                .into_response();
        }
        let sql = match s.mode {
            "readiness-delete" => Some(
                "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id=$1",
            ),
            "readiness-generation" => Some(
                "UPDATE zuse_rust_control_owners SET generation=generation+1 WHERE resource_id=$1",
            ),
            "readiness-lease" => {
                Some("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1")
            }
            "readiness-source" => Some(
                "UPDATE zuse_rust_allocations SET snapshot_id='replacement-source' WHERE workspace_id=$1",
            ),
            "readiness-expiry" => {
                Some("UPDATE zuse_rust_allocations SET boot_expires_at=0 WHERE workspace_id=$1")
            }
            _ => None,
        };
        if let Some(sql) = sql {
            sqlx::query(sql)
                .bind(&s.workspace)
                .execute(&s.pool)
                .await
                .unwrap();
        }
    }
    let mutation = match s.mode {
        "missing-receipt" => Some("request_config-'rustPositiveCreateReceipt'"),
        "forged-receipt" => Some(
            "jsonb_set(request_config,'{rustPositiveCreateReceipt,operationId}','\"forged-operation\"')",
        ),
        "stale-receipt" => {
            Some("jsonb_set(request_config,'{rustPositiveCreateReceipt,generation}','99')")
        }
        "source-receipt" => Some(
            "jsonb_set(request_config,'{rustPositiveCreateReceipt,sourceArtifactId}','\"other-source\"')",
        ),
        "incarnation-receipt" => Some(
            "jsonb_set(request_config,'{rustPositiveCreateReceipt,storageIncarnationId}','\"other-incarnation\"')",
        ),
        _ => None,
    };
    if let Some(expr) = mutation {
        sqlx::query(&format!(
            "UPDATE api_cloud_workspaces SET request_config={expr} WHERE workspace_id=$1"
        ))
        .bind(&s.workspace)
        .execute(&s.pool)
        .await
        .unwrap();
    }
    Json(json!({"sandbox":sandbox(&s)})).into_response()
}
async fn upload(axum::extract::State(s): axum::extract::State<FailedBoot>) -> Json<Value> {
    s.upload_count.fetch_add(1, Ordering::SeqCst);
    Json(json!({"ok":true}))
}
async fn snapshot(
    axum::extract::State(s): axum::extract::State<FailedBoot>,
) -> (axum::http::StatusCode, Json<Value>) {
    if s.mode == "source-rejected" {
        (
            axum::http::StatusCode::FORBIDDEN,
            Json(json!({"error":"private-provider-body-token"})),
        )
    } else {
        (
            axum::http::StatusCode::OK,
            Json(json!({"snapshot":{"name":"managed-artifact","status":"ready"}})),
        )
    }
}
async fn command(
    axum::extract::State(s): axum::extract::State<FailedBoot>,
    Json(body): Json<Value>,
) -> Json<Value> {
    assert!(
        body["command"]
            .as_str()
            .unwrap()
            .contains("sudo -n /usr/bin/python3")
    );
    let sql = match s.mode {
        "delete" => Some(
            "UPDATE api_cloud_workspaces SET desired_state='deleted',state='deleting' WHERE workspace_id=$1",
        ),
        "generation" => {
            Some("UPDATE zuse_rust_control_owners SET generation=generation+1 WHERE resource_id=$1")
        }
        "lease" => Some("UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=$1"),
        "source" => Some(
            "UPDATE zuse_rust_allocations SET snapshot_id='different-source' WHERE workspace_id=$1",
        ),
        "legacy" => Some(
            "UPDATE api_cloud_workspaces SET request_config=request_config-'rustPreparationVersion' WHERE workspace_id=$1",
        ),
        _ => None,
    };
    if let Some(sql) = sql {
        sqlx::query(sql)
            .bind(&s.workspace)
            .execute(&s.pool)
            .await
            .unwrap();
    }
    if s.mode == "retained" {
        let config: Value = sqlx::query_scalar(
            "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1",
        )
        .bind(&s.workspace)
        .fetch_one(&s.pool)
        .await
        .unwrap();
        *s.before_error_config.lock().unwrap() = Some(config);
    }
    Json(
        json!({"stdout":"private-helper-stdout-token","stderr":"private-helper-stderr-token","exitCode":if s.mode=="invalid-json"{0}else{1},"timedOut":false}),
    )
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn failed_boot_http_spans_survive_real_worker_error_and_respect_fences_without_secrets() {
    run_failed_boot_modes(&[
        "source-rejected",
        "missing-labels",
        "conflicting-labels",
        "conflicting-operation",
        "nonobject-labels",
        "name-only",
        "missing-receipt",
        "forged-receipt",
        "stale-receipt",
        "source-receipt",
        "incarnation-receipt",
        "helper-exit",
        "invalid-json",
        "delete",
        "generation",
        "lease",
        "source",
        "diagnostic-write-fails",
        "legacy",
        "retained",
    ])
    .await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn expected_readiness_wait_keeps_two_second_cadence_without_recreating_or_exhausting_budget()
{
    run_failed_boot_modes(&[
        "readiness-cadence",
        "readiness-starting",
        "readiness-stopped",
        "readiness-unknown",
        "readiness-budget",
        "readiness-503",
        "readiness-429",
        "readiness-delete",
        "readiness-generation",
        "readiness-lease",
        "readiness-source",
        "readiness-expiry",
    ])
    .await;
}

async fn run_failed_boot_modes(modes: &[&'static str]) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated database required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for &mode in modes {
        let schema = format!("failure_observation_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |c, _| {
                let scope = scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scope}"))
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for sql in [
            include_str!("../../../tests/fixtures/legacy-catalog.sql"),
            jobs::MIGRATION,
            crate::runtime::allocation::MIGRATION,
            crate::runtime::deletion::MIGRATION,
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        let workspace = format!("workspace_{}", uuid::Uuid::new_v4().simple());
        let spec = json!({"version":1,"mode":"post-hydration-first-start-v1","runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)});
        sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider='boat',provider_sandbox_id=NULL,runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,state='queued',desired_state='ready',runtime_state='offline',request_config=$2 WHERE workspace_id='ws-bob'").bind(&workspace).bind(json!({"runtimeGeneration":1,"gatewayEpoch":1,"rustProviderControllerVersion":1,"rustPreparationVersion":1,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"initialRuntimeBootstrap":spec,"managedReleaseId":"managed-release","untouched":"scoped-metadata","startupTimings":{"requestedAt":11}})).execute(&pool).await.unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',1,$2)")
            .bind(&workspace)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES($1,'user-bob','chat','session','turn','command','test-ciphertext',$2,$3)").bind(&workspace).bind(now+86400000).bind(now).execute(&pool).await.unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[74; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([74;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([75; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        );
        control
            .queue_allocation(&workspace, "managed-artifact", now)
            .await
            .unwrap();
        let expected_requested_at:Value=sqlx::query_scalar("SELECT request_config#>'{startupTimings,requestedAt}' FROM api_cloud_workspaces WHERE workspace_id=$1").bind(&workspace).fetch_one(&pool).await.unwrap();
        let expected_boot_hash: Option<String> = sqlx::query_scalar(
            "SELECT runtime_boot_token_hash FROM api_cloud_workspaces WHERE workspace_id=$1",
        )
        .bind(&workspace)
        .fetch_one(&pool)
        .await
        .unwrap();
        let control = if mode == "retained" {
            let pin:crate::managed_bases::candidate::StagingBasePin=serde_json::from_value(json!({"ownerId":"user-bob","provider":"boat","releaseId":"current-release","artifactId":"current-artifact","harnessRelease":"a".repeat(64),"manifestSha256":"b".repeat(64),"candidateSha256":"c".repeat(64),"expiresAt":now+86400000,"allowStagingCompute":true,"retainedWorkspaces":[{"workspaceId":workspace,"provider":"boat","releaseId":"old-release","artifactId":"old-artifact","harnessRelease":"a".repeat(64),"manifestSha256":"b".repeat(64),"candidateSha256":"d".repeat(64)}]})).unwrap();
            pin.deployment("staging", &["user-bob".into()], &["user-bob".into()], now)
                .unwrap();
            control.with_staging_base_pin(Some(Arc::new(pin)))
        } else {
            control
        };
        if mode == "diagnostic-write-fails" {
            sqlx::raw_sql("CREATE FUNCTION reject_diagnostic_write()RETURNS trigger LANGUAGE plpgsql AS $$BEGIN IF NEW.request_config#>'{startupTimings,providerPhases,initialBootstrapProofValidation}' IS NOT NULL THEN RAISE EXCEPTION 'private-diagnostic-storage-token';END IF;RETURN NEW;END$$;CREATE TRIGGER reject_diagnostic_write BEFORE UPDATE ON api_cloud_workspaces FOR EACH ROW EXECUTE FUNCTION reject_diagnostic_write()").execute(&pool).await.unwrap();
        }
        let create_count = Arc::new(AtomicUsize::new(0));
        let upload_count = Arc::new(AtomicUsize::new(0));
        let state = FailedBoot {
            pool: pool.clone(),
            workspace: workspace.clone(),
            mode,
            create: Arc::new(Mutex::new(None)),
            before_error_config: Arc::new(Mutex::new(None)),
            create_count: create_count.clone(),
            upload_count: upload_count.clone(),
        };
        let before_error_config = state.before_error_config.clone();
        let app = Router::new()
            .route("/api/v1/sandboxes", axum::routing::get(list).post(create))
            .route("/api/v1/sandboxes/{id}", axum::routing::get(get))
            .route("/api/v1/saved-snapshots/{id}", axum::routing::get(snapshot))
            .route("/api/v1/sandboxes/{id}/files", axum::routing::put(upload))
            .route(
                "/api/v1/sandboxes/{id}/commands",
                axum::routing::post(command),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut boat = Boat::new("private-provider-auth-token".into()).unwrap();
        boat.rest
            .test_endpoint(format!("http://{}/api/v1", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let adapter = RuntimeProvider::new(Arc::new(boat), "small".into());
        if mode.starts_with("readiness-") {
            let unexpected = matches!(mode, "readiness-stopped" | "readiness-unknown");
            let fenced = matches!(
                mode,
                "readiness-delete"
                    | "readiness-generation"
                    | "readiness-lease"
                    | "readiness-source"
                    | "readiness-expiry"
            );
            if mode == "readiness-budget" {
                sqlx::query("UPDATE zuse_rust_jobs SET attempts=119 WHERE resource_id=$1")
                    .bind(&workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let mut observations = Vec::new();
            for poll in 0..if fenced { 1 } else { 4 } {
                sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                    .bind(&workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
                let outcome = control
                    .allocate_once(&adapter, chrono::Utc::now().timestamp_millis())
                    .await;
                let row: Value = sqlx::query_scalar("SELECT jsonb_build_object('state',state,'attempts',attempts,'maxAttempts',max_attempts,'delay',due_at-updated_at,'lease',lease_token,'error',last_error_code) FROM zuse_rust_jobs WHERE resource_id=$1")
                    .bind(&workspace).fetch_one(&pool).await.unwrap();
                assert_eq!(
                    row["attempts"],
                    poll + 1 + if mode == "readiness-budget" { 119 } else { 0 },
                    "attempts stay monotonic"
                );
                if !fenced && !unexpected {
                    assert_eq!(
                        row["maxAttempts"].as_i64().unwrap() - row["attempts"].as_i64().unwrap(),
                        if mode == "readiness-budget" { 1 } else { 120 },
                        "expected polls preserve failure budget"
                    );
                }
                observations.push((outcome.is_ok(), row));
            }
            if matches!(mode, "readiness-429" | "readiness-503") {
                sqlx::query("UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id=$1")
                    .bind(&workspace)
                    .execute(&pool)
                    .await
                    .unwrap();
                assert!(
                    control
                        .allocate_once(&adapter, chrono::Utc::now().timestamp_millis())
                        .await
                        .is_err()
                );
                let delay: i64 = sqlx::query_scalar(
                    "SELECT due_at-updated_at FROM zuse_rust_jobs WHERE resource_id=$1",
                )
                .bind(&workspace)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(
                    delay, 2000,
                    "first transport failure keeps first-failure backoff despite expected polls"
                );
            }
            assert_eq!(create_count.load(Ordering::SeqCst), 1);
            assert_eq!(upload_count.load(Ordering::SeqCst), 0);
            server.abort();
            let _ = server.await;
            pool.close().await;
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&admin)
                .await
                .unwrap();
            if unexpected {
                assert!(
                    observations.iter().enumerate().all(|(index, (ok, row))| !ok
                        && row["state"] == "pending"
                        && row["maxAttempts"] == 120
                        && row["delay"] == 2000_i64 * (1_i64 << index)),
                    "unexpected states must retain exponential failure handling without budget credit: {observations:?}"
                );
                continue;
            }
            assert!(
                observations.iter().all(|(ok, row)| if fenced {
                    !ok
                } else {
                    *ok && row["state"] == "pending"
                        && row["lease"].is_null()
                        && row["delay"].as_i64().unwrap() <= 2000
                }),
                "successful-but-not-ready is a bounded observation, not exponential provider failure: {observations:?}"
            );
            continue;
        }
        let error = control.allocate_once(&adapter, now).await.unwrap_err();
        assert_eq!(
            error.code,
            if mode == "invalid-json" {
                "cloud_provider_invalid_response"
            } else if mode == "generation" {
                // The existing subsequent authority check supersedes the
                // provider error; diagnostics must not weaken that fence.
                "workspace_runtime_fenced"
            } else {
                "cloud_provider_rejected"
            },
            "{mode}"
        );
        let config: Value = sqlx::query_scalar(
            "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1",
        )
        .bind(&workspace)
        .fetch_one(&pool)
        .await
        .unwrap();
        let phases = &config["startupTimings"]["providerPhases"];
        assert_eq!(
            create_count.load(Ordering::SeqCst),
            usize::from(mode != "name-only"),
            "name-only reconciliation cannot create or mint positive receipt"
        );
        if matches!(
            mode,
            "conflicting-labels"
                | "conflicting-operation"
                | "nonobject-labels"
                | "name-only"
                | "missing-receipt"
                | "forged-receipt"
                | "stale-receipt"
                | "source-receipt"
                | "incarnation-receipt"
        ) {
            assert_eq!(
                phases["initialSourceVerification"]["lastResultOk"], false,
                "unproven or conflicting source must fail closed: {mode}"
            );
            assert!(
                phases.get("initialBootstrapHelper").is_none(),
                "no helper may execute for {mode}"
            );
        }
        let credentials: (Option<String>, Option<String>) = sqlx::query_as("SELECT runtime_credential_hash,runtime_boot_token_hash FROM api_cloud_workspaces WHERE workspace_id=$1").bind(&workspace).fetch_one(&pool).await.unwrap();
        assert_eq!(
            credentials,
            (None, expected_boot_hash),
            "server-side preallocated boot hash must remain unchanged: {mode}"
        );

        assert_eq!(
            upload_count.load(Ordering::SeqCst),
            0,
            "no private boot credential PUT before source/helper proof: {mode}"
        );
        let permitted = matches!(
            mode,
            "source-rejected" | "missing-labels" | "helper-exit" | "invalid-json"
        );
        if permitted {
            assert_eq!(
                phases["initialSourceVerification"]["lastResultOk"],
                mode != "source-rejected",
                "failed actual HTTP stage must survive worker error for {mode}"
            );
            if mode != "source-rejected" {
                assert_eq!(phases["initialBootstrapHelper"]["lastResultOk"], true);
                assert_eq!(
                    phases["initialBootstrapProofValidation"]["lastResultOk"],
                    false
                );
            }
        } else {
            assert!(
                phases.get("initialBootstrapProofValidation").is_none(),
                "late or failed diagnostic write cannot mutate {mode}"
            );
        }
        assert_eq!(config["untouched"], "scoped-metadata");
        assert_eq!(
            config["startupTimings"]["requestedAt"],
            expected_requested_at
        );
        if mode == "retained" {
            assert_eq!(
                Some(&config),
                before_error_config.lock().unwrap().as_ref(),
                "retained exemption config must remain byte-identical through actual failure path"
            );
        }
        for secret in [
            "private-provider-body-token",
            "private-provider-auth-token",
            "private-helper-stdout-token",
            "private-helper-stderr-token",
            "private-diagnostic-storage-token",
        ] {
            assert!(!config.to_string().contains(secret));
        }
        assert!(config["initialRuntimeBootstrapReceipt"].is_null());
        server.abort();
        let _ = server.await;
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
}
