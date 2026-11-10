//! Isolated PostgreSQL regression; the refresh driver is a gated fixture, not vendor acceptance.
use super::*;
use crate::auth::{Identity, Membership, MembershipRole, Principal};
use crate::connections::worker::{AuthDriver, AuthJob, DeviceChallenge, VerifiedConnection};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rsa::traits::PublicKeyParts;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Notify, mpsc};
use zeroize::Zeroizing;
struct MembershipGate(AtomicBool);
#[async_trait]
impl Identity for MembershipGate {
    async fn verify(&self, _: &str) -> Result<Principal, ApiError> {
        unreachable!()
    }
    async fn membership(&self, actor: &str, org: &str) -> Result<Membership, ApiError> {
        if self.0.load(Ordering::SeqCst) {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(Membership {
            id: "membership".into(),
            user_id: actor.into(),
            organization_id: org.into(),
            status: "active".into(),
            role: MembershipRole {
                slug: "member".into(),
            },
        })
    }
}
struct RefreshGate {
    entered: Notify,
    release: Notify,
}
#[async_trait]
impl AuthDriver for RefreshGate {
    async fn execute(
        &self,
        job: AuthJob,
        _: mpsc::Sender<DeviceChallenge>,
    ) -> Result<VerifiedConnection, &'static str> {
        assert!(job.refresh);
        assert_eq!(job.agent, "codex");
        self.entered.notify_one();
        self.release.notified().await;
        Ok(VerifiedConnection { credential: Zeroizing::new(json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":"fixture-rotated-access","refreshToken":"fixture-rotated-refresh","providerAccountId":"same-vendor-account","nativeAuth":{}}).to_string()), account_label: None, account_id: Some("same-vendor-account".into()), expires_at: Some(chrono::Utc::now().timestamp_millis()+3_600_000) })
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn organization_departure_during_refresh_commits_rotation_but_denies_grant() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("grant_scope_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |c, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={search}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../migrations/0001_control_plane.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,provider_sandbox_id text,runtime_credential_hash text,state text,desired_state text,request_config jsonb);CREATE TABLE cloud_agent_connections(connection_id text PRIMARY KEY,owner_id text,agent text,method text,state text,credential_ciphertext text,version bigint,authority_epoch bigint,expires_at bigint,refresh_lease_id text,refresh_lease_expires_at bigint,provider_account_id text,account_label text,last_error_code text,updated_at bigint);CREATE TABLE api_github_enrollments(organization_id text,account_id text,installation_id bigint,data jsonb);CREATE TABLE api_github_join_policies(organization_id text,installation_id bigint,data jsonb)").execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0024_credential_delivery_journal.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let encoded = B64.encode([71; 32]);
    let master = MasterKey::parse(&encoded).unwrap();
    let canonical=json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":"fixture-original-access","refreshToken":"fixture-original-refresh","providerAccountId":"same-vendor-account","nativeAuth":{}}).to_string();
    let ciphertext = master
        .seal_api("agent-connection:member:connection", &canonical)
        .unwrap();
    let authority = Arc::new(crate::connections::ConnectionControl::from_pool(
        pool.clone(),
        master,
        vec!["member".into()],
    ));
    let signer = ed25519_dalek::SigningKey::from_bytes(&[72; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([72;32]),"x":B64.encode(signer.verifying_key().as_bytes())});
    let control = Arc::new(
        RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&encoded).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        )
        .with_credential_authority(authority),
    );
    let identity = Arc::new(MembershipGate(AtomicBool::new(false)));
    control
        .configure_scope_authority(identity.clone(), None, vec!["team".into()])
        .unwrap();
    let rsa = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public=json!({"kty":"RSA","n":B64.encode(rsa.n().to_bytes_be()),"e":B64.encode(rsa.e().to_bytes_be())}).to_string();
    let thumb = credential_thumbprint(&public).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let config = json!({"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeGeneration":1,"gatewayEpoch":1,"runtimeBootstrapReceipt":{"workspaceId":"task","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+600000,"runtimeCredentialKeyThumbprint":thumb,"runtimeSigningKeyThumbprint":"signing-thumb","providerAuthMode":"broker-v1","rustActorId":"member","agentConnectionId":"connection","agentConnectionOwnerId":"member","rustRuntimeStorage":{"storageIncarnationId":"disk"},"rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"sandbox","storageIncarnationId":"disk","credentialKeyThumbprint":thumb,"signingKeyThumbprint":"signing-thumb"},"sharingPolicy":{"audience":"private","permission":"view","creatorSubject":"member","creatorMembershipId":"membership","grants":[]}});
    sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','task','rust',1,$1)")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_cloud_workspaces VALUES('task','organization:team','sandbox',$1,'ready','ready',$2)").bind(hash("runtime")).bind(config).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,agent,method,state,credential_ciphertext,version,authority_epoch,expires_at,provider_account_id,updated_at) VALUES('connection','member','codex','device-code','connected',$1,1,3,$2,'same-vendor-account',$3)").bind(ciphertext).bind(now+30_000).bind(now).execute(&pool).await.unwrap();
    let driver = Arc::new(RefreshGate {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let taskcontrol = control.clone();
    let taskdriver = driver.clone();
    let task = tokio::spawn(async move {
        taskcontrol
            .provider_grant_with_driver(
                "task",
                "runtime",
                CloudAuthProvider::Codex,
                ProviderGrantRequest {
                    protocol_version: 1,
                    request_id: uuid::Uuid::new_v4().to_string(),
                    runtime_generation: 1,
                    credential_public_jwk: public,
                    reason: zuse_protocol::cloud_auth::GrantReason::Initial,
                    previous_provider_account_id: None,
                    previous_chatgpt_account_id: None,
                },
                now,
                Some(taskdriver.as_ref()),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), driver.entered.notified())
        .await
        .unwrap();
    // No workspace transaction/lock is held while authority refresh waits.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "SELECT workspace_id FROM api_cloud_workspaces WHERE workspace_id='task' FOR UPDATE NOWAIT",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    identity.0.store(true, Ordering::SeqCst);
    control
        .invalidate_organization_access("team", "member")
        .await;
    driver.release.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .err()
        .expect("revoked member cannot receive grant");
    assert_eq!(error.code, "organization_access_denied");
    let saved:(i64,i64,String)=sqlx::query_as("SELECT version,authority_epoch,state FROM cloud_agent_connections WHERE connection_id='connection'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        saved,
        (2, 3, "connected".into()),
        "legitimate rotation must persist even though task lost access"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
