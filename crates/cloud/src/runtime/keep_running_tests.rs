//! Isolated PostgreSQL regression for the keep-running pass.
use super::*;
use crate::providers::{ExecResult, Machine, ProviderError, contract::*};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use sqlx::PgPool;
use std::{collections::BTreeSet, sync::Mutex};

#[derive(Default)]
struct Limited(Mutex<Vec<String>>);
#[async_trait::async_trait]
impl Provider for Limited {
    fn id(&self) -> ProviderId {
        ProviderId::E2b
    }
    fn has_running_limit(&self) -> bool {
        true
    }
    async fn keep_running(&self, id: &str) -> Result<(), ProviderError> {
        self.0.lock().unwrap().push(id.into());
        Ok(())
    }
    async fn inspect(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        unreachable!()
    }
    async fn find_allocation(&self, _: &str) -> Result<Option<Machine>, ProviderError> {
        unreachable!()
    }
    async fn allocate(&self, _: &Allocation<'_>) -> Result<Machine, ProviderError> {
        unreachable!()
    }
    async fn sleep(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        unreachable!()
    }
    async fn resume(&self, _: &str, _: u32) -> Result<ProviderOperation, ProviderError> {
        unreachable!()
    }
    async fn delete(&self, _: &str) -> Result<ProviderOperation, ProviderError> {
        unreachable!()
    }
    async fn operation(&self, _: &ProviderOperation) -> Result<OperationState, ProviderError> {
        unreachable!()
    }
    async fn execute(&self, _: &str, _: &str) -> Result<ExecResult, ProviderError> {
        unreachable!()
    }
    async fn upload(&self, _: &str, _: &str, _: &[u8]) -> Result<(), ProviderError> {
        unreachable!()
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn every_busy_computer_on_a_limited_provider_is_renewed_across_pages() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("keep_running_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
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
    sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,provider text,provider_sandbox_id text,state text,desired_state text,runtime_state text,last_activity_at bigint);CREATE TABLE zuse_rust_task_command_execution(workspace_id text,state text)").execute(&pool).await.unwrap();
    // 501 computers with a running task: more than one page.
    sqlx::raw_sql("INSERT INTO api_cloud_workspaces SELECT 'w'||lpad(i::text,4,'0'),'e2b','sbx-'||i,'ready','ready','online',0 FROM generate_series(1,501) i;INSERT INTO zuse_rust_task_command_execution SELECT 'w'||lpad(i::text,4,'0'),'running' FROM generate_series(1,501) i").execute(&pool).await.unwrap();
    // Skipped: idle with no task, asleep, and a provider without a limit.
    sqlx::raw_sql("INSERT INTO api_cloud_workspaces VALUES('x-idle','e2b','sbx-idle','ready','ready','online',0),('x-asleep','e2b','sbx-asleep','sleeping','sleeping','offline',0),('x-boxd','boxd','vm-busy','ready','ready','online',0);INSERT INTO zuse_rust_task_command_execution VALUES('x-asleep','running'),('x-boxd','running')").execute(&pool).await.unwrap();

    let encoded = B64.encode([71; 32]);
    let signer = ed25519_dalek::SigningKey::from_bytes(&[72; 32]);
    let jwk = serde_json::json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([72;32]),"x":B64.encode(signer.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        pool.clone(),
        crate::crypto::MasterKey::parse(&encoded).unwrap(),
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
            .unwrap(),
    );
    let limited = std::sync::Arc::new(Limited::default());
    let mut registry = Registry::default();
    registry.insert(limited.clone()).unwrap();

    let now = 10 * 86_400_000;
    assert_eq!(
        control
            .keep_running_registry_once(&registry, now)
            .await
            .unwrap(),
        501
    );
    let renewed: BTreeSet<String> = limited.0.lock().unwrap().iter().cloned().collect();
    assert_eq!(renewed.len(), 501, "each busy computer exactly once");
    assert!(renewed.contains("sbx-1") && renewed.contains("sbx-501"));
    assert!(!renewed.contains("sbx-idle") && !renewed.contains("sbx-asleep"));

    // Recent activity alone counts as busy.
    sqlx::query("UPDATE api_cloud_workspaces SET last_activity_at=$1 WHERE workspace_id='x-idle'")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    limited.0.lock().unwrap().clear();
    assert_eq!(
        control
            .keep_running_registry_once(&registry, now)
            .await
            .unwrap(),
        502
    );

    // No limited provider configured: no query, nothing renewed.
    assert_eq!(
        control
            .keep_running_registry_once(&Registry::default(), now)
            .await
            .unwrap(),
        0
    );
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
