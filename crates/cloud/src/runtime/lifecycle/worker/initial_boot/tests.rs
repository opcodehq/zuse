use super::*;
use crate::providers::Machine;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct Probe {
    pool: PgPool,
    mode: &'static str,
    calls: AtomicUsize,
    launches: AtomicUsize,
    original: Mutex<Option<Request>>,
}
#[async_trait]
impl RetainedProvider for Probe {
    fn provider_id(&self) -> &'static str {
        "boat"
    }
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(Some(Machine {
            id: id.into(),
            name: "retained-original".into(),
            state: MachineState::Running,
            vcpu: None,
            memory_bytes: None,
        }))
    }
    async fn retained_launch_ready(&self, _: &str) -> Result<bool, ProviderError> {
        Ok(self.mode != "not-hydrated")
    }
    async fn pause_retained(&self, _: &str) -> Result<(), ProviderError> {
        panic!("no pause")
    }
    async fn wake_retained(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        panic!("no replacement/wake")
    }
    async fn bootstrap_retained_runtime(&self, r: &Request) -> Result<bool, ProviderError> {
        assert!(r.purpose == Purpose::RetainedResume);
        assert_eq!(r.source_machine_id, "retained-source");
        assert_eq!(r.storage_incarnation_id, "retained-disk");
        {
            let mut prior = self.original.lock().unwrap();
            if let Some(prior) = prior.as_ref() {
                assert!(prior == r, "only original request reconciled")
            } else {
                *prior = Some(r.clone())
            }
        }
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query(
                "UPDATE api_cloud_workspaces SET revision=revision+1 WHERE workspace_id='ws-bob'",
            )
            .execute(&self.pool),
        )
        .await
        .expect("no SQL lock during helper")
        .unwrap();
        let sql = match self.mode {
            "delete" => Some(
                "UPDATE api_cloud_workspaces SET state='deleting',desired_state='deleted' WHERE workspace_id='ws-bob'",
            ),
            "lease-expired" => Some("UPDATE zuse_rust_jobs SET lease_expires_at=0"),
            "lease-replaced" => Some("UPDATE zuse_rust_jobs SET lease_token='replacement'"),
            "generation" => {
                Some("UPDATE zuse_rust_control_owners SET generation=4 WHERE resource_id='ws-bob'")
            }
            "source" => Some(
                "UPDATE api_cloud_workspaces SET provider_sandbox_id='wrong-source' WHERE workspace_id='ws-bob'",
            ),
            "disk" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRuntimeStorage,storageIncarnationId}','\"wrong-disk\"') WHERE workspace_id='ws-bob'",
            ),
            "operation" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustLifecycleOperation,id}','\"wrong-operation\"') WHERE workspace_id='ws-bob'",
            ),
            "pins" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{initialRuntimeBootstrap,helperSha256}',to_jsonb(repeat('f',64))) WHERE workspace_id='ws-bob'",
            ),
            "release" => Some(
                "UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{managedReleaseId}','\"wrong-release\"') WHERE workspace_id='ws-bob'",
            ),
            _ => None,
        };
        if let Some(sql) = sql {
            sqlx::query(sql).execute(&self.pool).await.unwrap();
        }
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.mode == "reply-lost" && n == 0 {
            return Err(ProviderError::OutcomeUnknown);
        }
        Ok(!(self.mode == "pending" && n == 0))
    }
    async fn launch_retained(&self, d: &Directive, w: &str, _: &str) -> Result<(), ProviderError> {
        assert_eq!(d.source_machine_id, "retained-source");
        assert_eq!(w, "ws-bob");
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn boat_disabled_unit_retained_start_is_hydration_and_current_owner_fenced() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    for mode in [
        "ready",
        "legacy",
        "not-hydrated",
        "pending",
        "reply-lost",
        "delete",
        "lease-expired",
        "lease-replaced",
        "generation",
        "source",
        "disk",
        "operation",
        "pins",
        "release",
        "one-checkout",
    ] {
        let schema = format!("boat_owned_start_{}", uuid::Uuid::new_v4().simple());
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
            include_str!("../../../../../tests/fixtures/legacy-catalog.sql"),
            include_str!("../../../../../tests/fixtures/billing.sql"),
            jobs::MIGRATION,
            crate::runtime::lifecycle::MIGRATION,
            include_str!("../../../../../migrations/0012_provider_receipts.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,overage_cap_micros,updated_at) VALUES('period','user-bob','active',0,9999999999999,0,1000000000,0,0,0)").execute(&pool).await.unwrap();
        let mut config = json!({"runtimeGeneration":3,"gatewayEpoch":3,"rustProviderControllerVersion":1,"managedReleaseId":"boat-explicit","rustRuntimeStorage":{"storageIncarnationId":"retained-disk"},"rustLifecycleOperation":{"id":"retained-operation","action":"resume","phase":"waking"}});
        if mode != "legacy" {
            config["initialRuntimeBootstrap"] = json!({"version":1,"mode":"post-hydration-first-start-v1","runtimeSha256":"a".repeat(64),"supervisorSha256":"b".repeat(64),"serviceSha256":"c".repeat(64),"manifestSha256":"d".repeat(64),"helperSha256":"e".repeat(64)});
        }
        if mode == "one-checkout" {
            config["rustMemorySleepVersion"] = json!(1);
            config["rustMemoryParked"] = json!({"version":1,"operationId":"prior-pause","generation":3,"sourceMachineId":"retained-source","storageIncarnationId":"retained-disk"});
            config["rustStorageEnrollment"] =
                json!({"capabilities":[zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY]});
        }
        sqlx::query("UPDATE api_cloud_workspaces SET provider='boat',provider_sandbox_id='retained-source',state='provisioning',desired_state='ready',runtime_state='offline',request_config=$1 WHERE workspace_id='ws-bob'").bind(config).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','ws-bob','rust',3,0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES('retained-operation','ws-bob','resume-command',3,'retained-source','retained-disk','resume','waking',1,1)").execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        jobs::enqueue(
            &mut tx,
            "retained-job",
            JOB_KIND,
            "ws-bob",
            3,
            &json!({"operationId":"retained-operation"}),
            clock_ms(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut one_pool = None;
        let control = if mode == "one-checkout" {
            let scope = schema.clone();
            let one = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(2))
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
            let c = super::super::tests::control(one.clone());
            one_pool = Some(one);
            c
        } else {
            super::super::tests::control(pool.clone())
        };
        let probe = Probe {
            pool: pool.clone(),
            mode,
            calls: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            original: Mutex::new(None),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            control.lifecycle_once(&probe, clock_ms()),
        )
        .await
        .expect("one-checkout route must not request a nested pool connection");
        let success = matches!(mode, "ready" | "legacy" | "one-checkout");
        if success {
            assert!(result.is_ok(), "{mode}: {result:?}");
            assert_eq!(probe.launches.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(probe.launches.load(Ordering::SeqCst), 0, "{mode}");
            if !matches!(mode, "not-hydrated" | "pending") {
                assert!(result.is_err(), "{mode}")
            }
        }
        assert_eq!(
            probe.calls.load(Ordering::SeqCst),
            if matches!(mode, "legacy" | "not-hydrated") {
                0
            } else {
                1
            }
        );
        if matches!(mode, "pending" | "reply-lost") {
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=0")
                .execute(&pool)
                .await
                .unwrap();
            control.lifecycle_once(&probe, clock_ms()).await.unwrap();
            assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
            assert_eq!(probe.launches.load(Ordering::SeqCst), 1);
        }
        if let Some(one) = one_pool {
            one.close().await;
        }
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
}
