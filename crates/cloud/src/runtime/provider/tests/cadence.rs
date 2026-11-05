//! Actual durable job/provider allocation: one positive create may advance the
//! same leased operation once, never claim another workspace or repeat create.
use super::*;

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn positive_create_continues_same_lease_once_and_rechecks_all_startup_fences() {
    run_cadence(&[
        "ready",
        "pending",
        "delete",
        "generation",
        "lease",
        "source",
        "machine",
        "wrong-operation",
        "capacity",
        "hold",
        "uncertain",
        "rejected",
    ])
    .await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn starting_create_inspects_original_running_machine_and_launches_in_same_call() {
    run_cadence(&["starting-ready"]).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn starting_create_inspects_still_starting_machine_once_without_private_boot() {
    run_cadence(&["starting-pending"]).await;
}

async fn run_cadence(modes: &[&str]) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated database required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for batched in [false, true] {
        for &mode in modes {
            let schema = format!("allocation_cadence_{}", uuid::Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE SCHEMA {schema}"))
                .execute(&admin)
                .await
                .unwrap();
            let scope = schema.clone();
            let pool = sqlx::postgres::PgPoolOptions::new()
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
                include_str!("../../../../tests/fixtures/legacy-catalog.sql"),
                jobs::MIGRATION,
                crate::runtime::allocation::MIGRATION,
                crate::runtime::deletion::MIGRATION,
            ] {
                sqlx::raw_sql(sql).execute(&pool).await.unwrap();
            }
            if batched {
                crate::runtime::statement_batch::fixture_schema(&pool).await;
            }
            let now = chrono::Utc::now().timestamp_millis();
            let workspace = format!("cadence-{mode}");
            sqlx::query("UPDATE api_cloud_workspaces SET workspace_id=$1,provider='boat',provider_sandbox_id=NULL,runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,state='queued',desired_state='ready',runtime_state='offline',request_config=$2 WHERE workspace_id='ws-bob'").bind(&workspace).bind(json!({"runtimeGeneration":1,"gatewayEpoch":1,"rustProviderControllerVersion":1})).execute(&pool).await.unwrap();
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
            let control = control.with_fast_start_accounts(if batched {
                vec!["user-bob".into()]
            } else {
                vec![]
            });
            assert_eq!(control.statement_batch_available().await.unwrap(), batched);
            let operation = control
                .queue_allocation(&workspace, "managed-artifact", now)
                .await
                .unwrap();
            // This real held allocation becomes overdue only after the target is
            // claimed. A second generic claim would consume it; continuation must not.
            sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id=NULL,runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,state='queued',desired_state='ready',runtime_state='offline',request_config=$1 WHERE workspace_id='ws-alice'").bind(json!({"runtimeGeneration":2,"gatewayEpoch":1,"rustProviderControllerVersion":1})).execute(&pool).await.unwrap();
            sqlx::query(
                "INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-alice','rust',2,$1)",
            )
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO api_cloud_workspace_launch_intents VALUES('ws-alice','user-alice','held-chat','held-session','held-turn','held-command','held-ciphertext',$1,$2)").bind(now+86400000).bind(now).execute(&pool).await.unwrap();
            control
                .queue_allocation("ws-alice", "managed-artifact", now)
                .await
                .unwrap();
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=request_config||'{\"rustRuntimeRepair\":{\"phase\":\"checking\"}}' WHERE workspace_id='ws-alice'").execute(&pool).await.unwrap();
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=$1 WHERE resource_id='ws-alice'")
                .bind(now + 60000)
                .execute(&pool)
                .await
                .unwrap();
            let mutation = match mode {
                "delete" => {
                    "UPDATE api_cloud_workspaces SET desired_state='deleted' WHERE workspace_id=NEW.workspace_id;"
                }
                "generation" => {
                    "UPDATE zuse_rust_control_owners SET generation=generation+1 WHERE resource_id=NEW.workspace_id;"
                }
                "lease" => {
                    "UPDATE zuse_rust_jobs SET lease_expires_at=0 WHERE resource_id=NEW.workspace_id;"
                }
                "source" => {
                    "UPDATE zuse_rust_allocations SET snapshot_id='replacement-source' WHERE workspace_id=NEW.workspace_id;"
                }
                "machine" => {
                    "UPDATE zuse_rust_allocations SET machine_id='replacement-resource' WHERE workspace_id=NEW.workspace_id;"
                }
                "wrong-operation" => {
                    "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustAllocationOperationId}','\"replacement-operation\"') WHERE workspace_id=NEW.workspace_id;"
                }
                "capacity" => {
                    "UPDATE api_entitlements SET status='revoked' WHERE account_id='user-bob'; UPDATE api_cloud_workspaces SET request_config=request_config||'{\"rustCreateRequestDigest\":\"was-authorized\"}' WHERE workspace_id=NEW.workspace_id;"
                }
                "hold" => {
                    "UPDATE api_cloud_workspaces SET request_config=request_config||'{\"rustRuntimeRepair\":{\"phase\":\"checking\"}}' WHERE workspace_id=NEW.workspace_id;"
                }
                _ => "",
            };
            let trigger = format!(
                "CREATE FUNCTION cadence_after_persist() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.workspace_id='{workspace}' AND NEW.provider_sandbox_id='original-resource' AND OLD.provider_sandbox_id IS DISTINCT FROM NEW.provider_sandbox_id THEN UPDATE zuse_rust_jobs SET due_at=0 WHERE resource_id='ws-alice'; {mutation} END IF; RETURN NEW; END $$; CREATE TRIGGER cadence_after_persist AFTER UPDATE ON api_cloud_workspaces FOR EACH ROW EXECUTE FUNCTION cadence_after_persist();"
            );
            sqlx::raw_sql(&trigger).execute(&pool).await.unwrap();
            let fixture = Arc::new(Fixture {
                id: ProviderId::Boat,
                pool: pool.clone(),
                workspace: workspace.clone(),
                operation: Mutex::new(operation.clone()),
                machine: Mutex::new(None),
                finds: AtomicUsize::new(0),
                creates: AtomicUsize::new(0),
                uploads: AtomicUsize::new(0),
                inspections: AtomicUsize::new(0),
                launches: AtomicUsize::new(0),
                seen_leases: Mutex::new(vec![]),
                token: Mutex::new(vec![]),
                readiness: Mutex::new(format!("immediate-{mode}")),
            });
            let mut registry = Registry::default();
            registry.insert(fixture.clone()).unwrap();
            let result = control.allocate_registry_once(&registry, now).await;
            assert_eq!(
                fixture.creates.load(Ordering::SeqCst),
                1,
                "{mode}: never create another resource"
            );
            let mut starting_failure = None;
            if mode.starts_with("starting-") {
                let ready = mode == "starting-ready";
                let result_ok = matches!(&result, Ok(true));
                let seen = fixture.seen_leases.lock().unwrap();
                let same_lease = !seen.is_empty() && seen.iter().all(|value| value == &seen[0]);
                let phase: String = sqlx::query_scalar(
                    "SELECT phase FROM zuse_rust_allocations WHERE operation_id=$1",
                )
                .bind(&operation)
                .fetch_one(&pool)
                .await
                .unwrap();
                let job: Value = sqlx::query_scalar("SELECT jsonb_build_object('state',state,'lease',lease_token,'attempts',attempts) FROM zuse_rust_jobs WHERE resource_id=$1").bind(&workspace).fetch_one(&pool).await.unwrap();
                if !result_ok
                    || fixture.inspections.load(Ordering::SeqCst) != 1
                    || fixture.launches.load(Ordering::SeqCst) != usize::from(ready)
                    || fixture.uploads.load(Ordering::SeqCst) != usize::from(ready)
                    || !same_lease
                    || phase != if ready { "launching" } else { "allocated" }
                    || job["state"] != "pending"
                    || !job["lease"].is_null()
                {
                    starting_failure = Some(format!(
                        "mode={mode}, result={result:?}, inspections={}, launches={}, uploads={}, same_lease={same_lease}, phase={phase}, job={job}",
                        fixture.inspections.load(Ordering::SeqCst),
                        fixture.launches.load(Ordering::SeqCst),
                        fixture.uploads.load(Ordering::SeqCst)
                    ));
                }
            } else if mode == "ready" {
                assert!(result.unwrap());
                assert_eq!(
                    fixture.launches.load(Ordering::SeqCst),
                    1,
                    "positive durable create should launch in this SAME top-level worker call"
                );
                assert_eq!(fixture.inspections.load(Ordering::SeqCst), 1);
                assert_eq!(fixture.uploads.load(Ordering::SeqCst), 1);
                let seen = fixture.seen_leases.lock().unwrap();
                assert!(seen.len() >= 5);
                assert!(
                    seen.iter().all(|v| v == &seen[0]),
                    "same durable job/lease/generation through create and launch"
                );
                let phase: String = sqlx::query_scalar(
                    "SELECT phase FROM zuse_rust_allocations WHERE operation_id=$1",
                )
                .bind(&operation)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(phase, "launching");
            } else {
                assert_eq!(
                    fixture.launches.load(Ordering::SeqCst),
                    0,
                    "{mode}: no launch"
                );
                assert_eq!(
                    fixture.uploads.load(Ordering::SeqCst),
                    0,
                    "{mode}: no private boot upload"
                );
                assert_eq!(
                    fixture.inspections.load(Ordering::SeqCst),
                    usize::from(mode == "pending"),
                    "{mode}: fresh fences before inspect; readiness never spins"
                );
            }
            let held:Value=sqlx::query_scalar("SELECT jsonb_build_object('state',state,'attempts',attempts,'lease',lease_token) FROM zuse_rust_jobs WHERE resource_id='ws-alice'").fetch_one(&pool).await.unwrap();
            assert_eq!(
                held,
                json!({"state":"pending","attempts":0,"lease":null}),
                "unrelated held job cannot be reclaimed during targeted continuation"
            );
            drop(registry);
            drop(fixture);
            drop(control);
            pool.close().await;
            sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(&admin)
                .await
                .unwrap();
            assert!(
                starting_failure.is_none(),
                "positive Starting receipt must inspect original once without waiting for worker cycle: {starting_failure:?}"
            );
        }
    }
}
