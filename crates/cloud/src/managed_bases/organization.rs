//! Isolated contract fixture: real SQL admission/bootstrap/authorization, fake
//! infrastructure transport. This is not live provider or model certification.
use super::*;
use crate::{
    access::{Access, Scope},
    auth::{Identity, Membership, MembershipRole, Principal},
    crypto::MasterKey,
    objects::Objects,
    providers::{Machine, MachineState, ProviderError},
    runtime::{
        RuntimeControl,
        allocation::{AllocationProvider, RuntimeLaunch},
    },
    tickets::TicketSigner,
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rsa::traits::PublicKeyParts;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
const OWNER: &str = "organization:org-team";
struct IdentityFixture {
    removed: AtomicBool,
}
#[async_trait]
impl Identity for IdentityFixture {
    async fn verify(&self, _: &str) -> Result<Principal, ApiError> {
        unreachable!()
    }
    async fn membership(&self, actor: &str, organization: &str) -> Result<Membership, ApiError> {
        if organization != "org-team"
            || (actor == "user-member" && self.removed.load(Ordering::SeqCst))
        {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(member(actor))
    }
}
fn member(actor: &str) -> Membership {
    Membership {
        id: format!("membership-{actor}"),
        user_id: actor.into(),
        organization_id: "org-team".into(),
        status: "active".into(),
        role: MembershipRole {
            slug: if actor == "user-admin" {
                "admin"
            } else if actor == "user-billing" {
                "billing"
            } else {
                "member"
            }
            .into(),
        },
    }
}
fn access(actor: &str) -> Access {
    Access {
        actor: Principal {
            account_id: actor.into(),
        },
        scope: Scope::Organization {
            organization_id: "org-team".into(),
        },
        owner_id: OWNER.into(),
        membership: Some(member(actor)),
    }
}
#[derive(Default)]
struct MemoryObjects(Mutex<std::collections::BTreeMap<String, Vec<u8>>>);
#[async_trait]
impl Objects for MemoryObjects {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let mut objects = self.0.lock().unwrap();
        if let Some(previous) = objects.get(key) {
            assert_eq!(previous, bytes);
        } else {
            objects.insert(key.into(), bytes.to_vec());
        }
        Ok(())
    }
    fn supports_purge(&self) -> bool {
        true
    }
    async fn purge_prefix(&self, prefix: &str) -> Result<bool, ApiError> {
        self.0
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(prefix));
        Ok(true)
    }
}
#[derive(Default)]
struct Provider {
    machines: Mutex<std::collections::BTreeMap<String, Machine>>,
    launches: Mutex<std::collections::BTreeMap<String, (String, String, i64, String)>>,
    wakes: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl AllocationProvider for Provider {
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        Ok(self
            .machines
            .lock()
            .unwrap()
            .values()
            .find(|machine| machine.id == name || machine.name == name)
            .cloned())
    }
    async fn create(&self, _: &str, name: &str) -> Result<Machine, ProviderError> {
        // Provider names are stable per allocation operation; retries return the
        // same resource, while the second pilot computer gets a distinct ID.
        if let Some(machine) = self.find(name).await? {
            return Ok(machine);
        }
        let machine = Machine {
            id: format!("org-sandbox-{name}"),
            name: name.into(),
            state: MachineState::Running,
            vcpu: Some(2),
            memory_bytes: Some(2 * 1024 * 1024 * 1024),
        };
        self.machines
            .lock()
            .unwrap()
            .insert(machine.id.clone(), machine.clone());
        Ok(machine)
    }
    async fn launch(&self, machine: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError> {
        self.launches.lock().unwrap().insert(
            launch.workspace_id.clone(),
            (
                launch.boot_token.to_string(),
                launch.incarnation.clone(),
                launch.generation,
                machine.to_owned(),
            ),
        );
        Ok(())
    }
}
#[async_trait]
impl crate::runtime::lifecycle::RetainedProvider for Provider {
    async fn inspect_retained(&self, machine: &str) -> Result<Option<Machine>, ProviderError> {
        self.find(machine).await
    }
    async fn pause_retained(&self, _: &str) -> Result<(), ProviderError> {
        Ok(())
    }
    async fn wake_retained(&self, _: &str, _: &MachineState) -> Result<(), ProviderError> {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn launch_retained(
        &self,
        _: &zuse_protocol::cloud_lifecycle::Directive,
        _: &str,
        _: &str,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
}
async fn database() -> (PgPool, PgPool, String) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("explicit isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("org_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for migration in [
        include_str!("../../migrations/0000_application.sql"),
        include_str!("../../migrations/0001_control_plane.sql"),
        include_str!("../../migrations/0002_gateway_leases.sql"),
        include_str!("../../migrations/0003_command_mailbox.sql"),
        include_str!("../../migrations/0004_allocations.sql"),
        include_str!("../../migrations/0005_lifecycle.sql"),
        include_str!("../../migrations/0006_recovery.sql"),
        include_str!("../../migrations/0007_images.sql"),
        include_str!("../../migrations/0007_restore.sql"),
        include_str!("../../migrations/0008_delete.sql"),
        include_str!("../../migrations/0008_launch_assets.sql"),
        include_str!("../../migrations/0009_computers.sql"),
        include_str!("../../migrations/0010_managed_bases.sql"),
        include_str!("../../migrations/0011_transcript_heads.sql"),
        include_str!("../../migrations/0012_provider_receipts.sql"),
        include_str!("../../migrations/0013_recovery_receipts.sql"),
        include_str!("../../migrations/0014_restore_receipts.sql"),
        include_str!("../../migrations/0015_github.sql"),
        include_str!("../../migrations/0016_task_history.sql"),
        include_str!("../../migrations/0019_workload_secrets.sql"),
        include_str!("../../migrations/0020_staging_managed_base_candidates.sql"),
        include_str!("../../migrations/0023_managed_base_admission_lock.sql"),
        include_str!("../../migrations/0025_admission_read_batch.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    (admin, pool, schema)
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn organization_execution_admission_allocation_bootstrap_lifecycle_and_actor_fences() {
    run_organization_fixture(false).await;
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn organization_candidate_preserves_actor_personal_connection_scope_and_worker_fences() {
    run_organization_fixture(true).await;
}
async fn run_organization_fixture(candidate_mode: bool) {
    let (admin, pool, schema) = database().await;
    let now = chrono::Utc::now().timestamp_millis();
    if !candidate_mode {
        sqlx::query("INSERT INTO api_entitlements(entitlement_id,account_id,kind,provider,status,paid_through,created_at,updated_at) VALUES('ent',$1,'cloud-workspace','polar','active',$2,$3,$3)").bind(OWNER).bind(now+86400000).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,base_price_micros,included_provider_cost_micros,markup_basis_points,price_catalog_version,overage_cap_micros,created_at,updated_at) VALUES('period',$1,'manual',$2,$3,0,5000000,0,'fixture',0,$2,$2)").bind(OWNER).bind(now-1000).bind(now+86400000).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('project',$1,'github.com/example/repo','https://github.com/example/repo.git','repo','main','public','github-app','fixture','connected','project-key',$2,$2)").bind(OWNER).bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,created_at,updated_at) VALUES('connection','user-member','auth-key','fixture','codex','api-key','connected',$1,$1)").bind(now).execute(&pool).await.unwrap();
    let key = B64.encode([64; 32]);
    let signing = ed25519_dalek::SigningKey::from_bytes(&[65; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([65;32]),"x":B64.encode(signing.verifying_key().as_bytes())});
    let worker_signer =
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap();
    let authority = Arc::new(crate::connections::ConnectionControl::from_pool(
        pool.clone(),
        MasterKey::parse(&key).unwrap(),
        vec!["user-member".into(), OWNER.into()],
    ));
    let objects = Arc::new(MemoryObjects::default());
    let mut control = RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&key).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
    .with_creation_accounts(vec![OWNER.into()])
    .with_credential_authority(authority.clone())
    .with_transcript_objects(objects);
    let identity = Arc::new(IdentityFixture {
        removed: AtomicBool::new(false),
    });
    let github =
        crate::github::native::admission_fixture::connected(&pool, OWNER, "user-member").await;
    control
        .configure_scope_authority(
            identity.clone(),
            Some(github.github.clone()),
            vec!["org-team".into()],
        )
        .unwrap();
    let mut store = crate::store::Store::from_pool(pool.clone());
    store.connections = Some(authority);
    let body = || {
        serde_json::from_value::<crate::runtime::creation::CreateWorkspace>(json!({"projectId":"project","providerId":"boxd","baseRef":"main","branch":"task/org","agent":"codex","model":"fixture","agentConnectionId":"connection","firstMessage":"Organization task","idempotencyKey":"org-task-key"})).unwrap()
    };
    let mut original = super::tests::release();
    original.manifest.rust_github_credentials_version = Some(1);
    original.register(&pool, now).await.unwrap();
    assert!(
        control
            .create_workspace(&access("user-member"), body(), now)
            .await
            .is_err(),
        "uncertified org base must not allocate"
    );
    assert!(
        !store
            .computer_capabilities(OWNER, now)
            .await
            .unwrap()
            .providers
            .iter()
            .any(|provider| provider.can_create())
    );
    let mut certified = original.clone();
    certified.release_id = "managed-org-fixture-v1".into();
    certified.artifact_id = "platform-org-snapshot".into();
    certified.manifest.build_id = certified.release_id.clone();
    certified.manifest.rust_organization_execution_version = Some(1);
    let pin = if candidate_mode {
        let c = candidate::ManagedBaseCandidate {
            provider: certified.provider,
            release_id: certified.release_id.clone(),
            artifact_id: certified.artifact_id.clone(),
            provider_artifact_binding: None,
            initial_runtime_bootstrap: None,
            architecture: certified.architecture.clone(),
            capabilities: certified.capabilities.clone(),
            manifest: certified.manifest.clone(),
            harnesses: certified.harnesses.clone(),
            harness_release: "d".repeat(64),
            runtime_sha256: "e".repeat(64),
            provenance_report_sha256: "f".repeat(64),
        };
        let pin = Arc::new(
            c.pin_for_actor(OWNER, "user-member", now + 60000, true)
                .unwrap(),
        );
        pin.deployment(
            "staging",
            &[OWNER.into()],
            &[OWNER.into(), "user-member".into()],
            now,
        )
        .unwrap();
        c.stage(&pool, &pin, now).await.unwrap();
        control = control.with_staging_base_pin(Some(pin.clone()));
        store = store.with_staging_base_pin(Some(pin.clone()));
        Some(pin)
    } else {
        certified.register(&pool, now + 1).await.unwrap();
        None
    };
    assert!(
        store
            .computer_capabilities(OWNER, now)
            .await
            .unwrap()
            .providers
            .iter()
            .any(|provider| provider.can_create())
    );
    let connected = store.agent_connections("user-member").await.unwrap();
    let catalog = store
        .provider_launch_catalogs_for(OWNER, &connected)
        .await
        .unwrap();
    assert_eq!(catalog["boxd"].providers.len(), 1);
    assert!(catalog["boat"].providers.is_empty());
    let chunk = |key: &str| zuse_protocol::cloud_assets::Chunk {
        idempotency_key: key.into(),
        upload_id: "upload".into(),
        seq: 0,
        bytes: B64.encode(b"org"),
    };
    let first = control
        .launch_asset_chunk(&access("user-member"), chunk("upload-key"), now)
        .await
        .unwrap();
    let second = control
        .launch_asset_chunk(&access("user-other"), chunk("upload-key"), now)
        .await;
    if candidate_mode {
        assert_eq!(
            second.unwrap_err().code,
            "staging_candidate_actor_not_selected"
        );
    } else {
        assert_ne!(first["workspaceId"], second.unwrap()["workspaceId"]);
    }
    assert!(
        control
            .launch_asset_chunk(&access("user-billing"), chunk("billing-upload"), now)
            .await
            .is_err()
    );
    if candidate_mode {
        let mut wrong = body();
        wrong.idempotency_key = "wrong-actor-key".into();
        wrong.branch = Some("task/wrong-actor".into());
        assert_eq!(
            control
                .create_workspace(&access("user-other"), wrong, now)
                .await
                .unwrap_err()
                .code,
            "staging_candidate_actor_not_selected"
        );
        assert!(
            !store
                .computer_capabilities_for_actor(OWNER, "user-other", now)
                .await
                .unwrap()
                .can_allocate
        );
        assert!(
            store
                .computer_capabilities_for_actor(OWNER, "user-member", now)
                .await
                .unwrap()
                .can_allocate
        );
    }
    let (created, new) = control
        .create_workspace(&access("user-member"), body(), now)
        .await
        .unwrap();
    assert!(new);
    let id = created["workspace"]["workspaceId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut expected_allocations = vec![id.clone()];
    let config: Value =
        sqlx::query_scalar("SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(config["rustActorId"], "user-member");
    assert_eq!(config["agentConnectionOwnerId"], "user-member");
    assert_eq!(config["rustOrganizationExecutionVersion"], 1);
    if let Some(pin) = &pin {
        assert_eq!(config["managedCandidateSha256"], pin.candidate_sha256);
        assert_eq!(config["managedBaseReadiness"], "uncertified-staging");
        let mut second = body();
        second.idempotency_key = "second-candidate".into();
        second.branch = Some("task/second-candidate".into());
        let (second_created, second_new) = control
            .create_workspace(&access("user-member"), second, now)
            .await
            .unwrap();
        assert!(second_new);
        expected_allocations.push(
            second_created["workspace"]["workspaceId"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        assert_ne!(
            second_created["workspace"]["workspaceId"],
            created["workspace"]["workspaceId"]
        );
        let mut third = body();
        third.idempotency_key = "third-candidate".into();
        third.branch = Some("task/third-candidate".into());
        assert_eq!(
            control
                .create_workspace(&access("user-member"), third, now)
                .await
                .unwrap_err()
                .code,
            "staging_candidate_computer_limit"
        );
        let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM api_entitlements),(SELECT count(*) FROM api_cloud_billing_periods),(SELECT count(*) FROM cloud_managed_bases WHERE state='verified' AND runtime_manifest->'manifest'->>'rustOrganizationExecutionVersion'='1')").fetch_one(&pool).await.unwrap();
        assert_eq!(
            counts,
            (0, 0, 0),
            "candidate must not fabricate billing or certified org support"
        );
    }

    assert_eq!(
        control
            .create_workspace(&access("user-other"), body(), now)
            .await
            .unwrap_err()
            .code,
        "cloud_workspace_idempotency_conflict"
    );
    assert_eq!(
        control
            .launch_asset_chunk(&access("user-other"), chunk("org-task-key"), now)
            .await
            .unwrap_err()
            .code,
        if candidate_mode {
            "staging_candidate_actor_not_selected"
        } else {
            "cloud_workspace_idempotency_conflict"
        }
    );
    let worker_proof = worker_signer
        .allocation_scope_proof(
            &id,
            config["rustAllocationOperationId"].as_str().unwrap(),
            config["runtimeGeneration"].as_i64().unwrap(),
            now,
        )
        .unwrap();
    control
        .authorize_allocation_scope(&id, &worker_proof, now)
        .await
        .unwrap();
    assert!(
        control
            .authorize_allocation_scope("other-task", &worker_proof, now)
            .await
            .is_err()
    );
    let provider = Provider::default();
    for round in 0..8 {
        control
            .allocate_once(&provider, now + round * 1000)
            .await
            .unwrap();
        if expected_allocations
            .iter()
            .all(|workspace| provider.launches.lock().unwrap().contains_key(workspace))
        {
            break;
        }
    }
    assert_eq!(
        provider.launches.lock().unwrap().len(),
        expected_allocations.len()
    );
    let machine_ids: std::collections::BTreeSet<_> = provider
        .launches
        .lock()
        .unwrap()
        .values()
        .map(|launch| launch.3.clone())
        .collect();
    assert_eq!(
        machine_ids.len(),
        expected_allocations.len(),
        "each admitted task has its own provider resource"
    );
    let (boot, incarnation, generation, machine_id) = provider
        .launches
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .expect("the original authorized org task reaches its own provider launch");
    let stored_machine: String = sqlx::query_scalar(
        "SELECT provider_sandbox_id FROM api_cloud_workspaces WHERE workspace_id=$1",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        machine_id, stored_machine,
        "bootstrap uses the original task's launch receipt"
    );
    let rsa = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public=json!({"kty":"RSA","n":B64.encode(rsa.n().to_bytes_be()),"e":B64.encode(rsa.e().to_bytes_be())}).to_string();
    let guest_signing = ed25519_dalek::SigningKey::from_bytes(&[66; 32]);
    let guest_public=json!({"kty":"OKP","crv":"Ed25519","x":B64.encode(guest_signing.verifying_key().as_bytes())}).to_string();
    let caps = vec![
        "rust-rpc-v1".to_owned(),
        "rust-session-commands-v1".into(),
        "rust-workspace-lifecycle-v1".into(),
        "rust-memory-sleep-v1".into(),
        "rust-transcript-projection-v1".into(),
        "rust-full-state-recovery-v1".into(),
        "rust-organization-execution-v1".into(),
    ];
    let request = |capabilities| crate::runtime::BootstrapRequest {
        credential_public_jwk: public.clone(),
        signing_public_jwk: guest_public.clone(),
        capabilities: Some(capabilities),
        rpc_protocol: Some(zuse_protocol::cloud_gateway::RUST_RPC.into()),
    };
    assert_eq!(
        control
            .bootstrap(&id, &boot, request(vec!["rust-rpc-v1".into()]), now)
            .await
            .unwrap_err()
            .code,
        "workspace_runtime_update_required"
    );
    let bootstrap = control
        .bootstrap(&id, &boot, request(caps.clone()), now)
        .await
        .unwrap();
    assert_eq!(bootstrap["actorId"], "user-member");
    assert_eq!(bootstrap["rustOrganizationExecutionVersion"], 1);
    let credential = bootstrap["runtimeCredential"].as_str().unwrap().to_owned();
    let enrollment = control
        .enroll_storage(
            &id,
            &credential,
            zuse_protocol::cloud::CloudStorageEnrollment {
                phase: zuse_protocol::cloud::CloudStoragePhase::Prepare,
                runtime_generation: generation,
                gateway_epoch: 1,
                storage_incarnation_id: incarnation.clone(),
                enrollment_id: None,
                capabilities: None,
            },
            now,
        )
        .await
        .unwrap();
    let enrollment_id = enrollment["enrollmentId"].as_str().unwrap().to_owned();
    control
        .enroll_storage(
            &id,
            &credential,
            zuse_protocol::cloud::CloudStorageEnrollment {
                phase: zuse_protocol::cloud::CloudStoragePhase::Commit,
                runtime_generation: generation,
                gateway_epoch: 1,
                storage_incarnation_id: incarnation,
                enrollment_id: Some(enrollment_id),
                capabilities: Some(caps),
            },
            now,
        )
        .await
        .unwrap();
    control
        .acknowledge(
            &id,
            &credential,
            crate::runtime::BootstrapAck {
                runtime_generation: generation,
                gateway_epoch: 1,
            },
            now,
        )
        .await
        .unwrap();
    control
        .ready_launch(
            &id,
            &credential,
            zuse_protocol::cloud::CloudRuntimeReady {
                phase: "agent-started".into(),
                launch_command_id: Some(
                    bootstrap["launchIntent"]["commandId"]
                        .as_str()
                        .unwrap()
                        .into(),
                ),
                session_head_version: Some(0),
            },
            now,
        )
        .await
        .unwrap();
    let ticket = control
        .gateway_ticket(
            &id,
            &access("user-member"),
            zuse_protocol::cloud_gateway::RUST_PROTOCOL,
            Some(zuse_protocol::cloud_gateway::RUST_RPC),
            "device",
            now,
        )
        .await
        .unwrap();
    let grant = control
        .gateway_authorize(
            &id,
            zuse_protocol::cloud_gateway::RUST_PROTOCOL,
            ticket["credential"].as_str().unwrap(),
            now,
        )
        .await
        .unwrap();
    assert_eq!(grant.actor, "user-member");
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{sharingPolicy,grants}','[{\"membershipId\":\"membership-user-viewer\",\"permission\":\"view\"}]') WHERE workspace_id=$1").bind(&id).execute(&pool).await.unwrap();
    assert_eq!(
        control
            .lifecycle_action(
                &id,
                &access("user-viewer"),
                zuse_protocol::cloud_lifecycle::Action::Pause,
                Some("view-pause"),
                now
            )
            .await
            .unwrap_err()
            .code,
        "workspace_access_denied"
    );
    assert_eq!(
        control
            .delete_workspace(&id, &access("user-viewer"), Some("view-delete"), now)
            .await
            .unwrap_err()
            .code,
        "workspace_access_denied"
    );
    control
        .lifecycle_action(
            &id,
            &access("user-member"),
            zuse_protocol::cloud_lifecycle::Action::Pause,
            Some("member-pause"),
            now,
        )
        .await
        .unwrap();
    // Simulate the controller's completed sleep receipt, then queue a real
    // retained Resume. Eligibility is checked again at worker execution time.
    sqlx::query("UPDATE zuse_rust_lifecycle_operations SET phase='paused' WHERE workspace_id=$1")
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state='paused',request_config=jsonb_set(request_config,'{rustLifecycleOperation,phase}','\"paused\"') WHERE workspace_id=$1").bind(&id).execute(&pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_jobs SET state='completed',lease_token=NULL,lease_expires_at=NULL WHERE resource_id=$1 AND kind='workspace-lifecycle'").bind(&id).execute(&pool).await.unwrap();
    control
        .lifecycle_action(
            &id,
            &access("user-member"),
            zuse_protocol::cloud_lifecycle::Action::Resume,
            Some("retained-resume"),
            now,
        )
        .await
        .unwrap();
    let resume:String=sqlx::query_scalar("SELECT request_config->'rustLifecycleOperation'->>'id' FROM api_cloud_workspaces WHERE workspace_id=$1").bind(&id).fetch_one(&pool).await.unwrap();
    let resume_proof = worker_signer
        .allocation_scope_proof(&id, &format!("lifecycle:{resume}"), generation, now)
        .unwrap();
    control
        .authorize_worker_effect_scope(&id, &resume_proof, now)
        .await
        .unwrap();
    let wrong_restore = worker_signer
        .allocation_scope_proof(&id, &format!("restore:{resume}"), generation, now)
        .unwrap();
    assert!(
        control
            .authorize_worker_effect_scope(&id, &wrong_restore, now)
            .await
            .is_err()
    );
    identity.removed.store(true, Ordering::SeqCst);
    control
        .invalidate_organization_access("org-team", "user-member")
        .await;
    assert!(
        control
            .authorize_worker_effect_scope(&id, &resume_proof, now)
            .await
            .is_err()
    );
    assert!(control.lifecycle_once(&provider, now).await.is_err());
    assert_eq!(
        provider.wakes.load(Ordering::SeqCst),
        0,
        "departure must prevent delayed provider wake"
    );
    assert!(
        control
            .gateway_ticket(
                &id,
                &access("user-member"),
                zuse_protocol::cloud_gateway::RUST_PROTOCOL,
                Some(zuse_protocol::cloud_gateway::RUST_RPC),
                "device",
                now
            )
            .await
            .is_err()
    );
    assert!(
        control
            .lifecycle_action(
                &id,
                &access("user-member"),
                zuse_protocol::cloud_lifecycle::Action::Resume,
                Some("removed-resume"),
                now
            )
            .await
            .is_err()
    );
    assert!(
        control
            .authorize_allocation_scope(&id, &worker_proof, now)
            .await
            .is_err(),
        "departed actor cannot authorize worker effects"
    );
    control
        .delete_workspace(&id, &access("user-admin"), Some("admin-delete"), now)
        .await
        .unwrap();
    let desired: String =
        sqlx::query_scalar("SELECT desired_state FROM api_cloud_workspaces WHERE workspace_id=$1")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(desired, "deleted");
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
