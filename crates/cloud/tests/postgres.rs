//! Explicit opt-in only: these tests create a private schema in a localhost database.
use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
use zuse_cloud::{
    auth::{Identity, Membership, MembershipRole, Principal},
    error::ApiError,
    jobs,
    routes::{AppState, router},
    store::Store,
};

struct TestIdentity;
#[async_trait]
impl Identity for TestIdentity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if !["alice", "bob", "admin", "member", "billing"].contains(&token) {
            return Err(ApiError::unauthorized("invalid_workos_token"));
        }
        Ok(Principal {
            account_id: format!("user-{token}"),
        })
    }
    async fn membership(&self, account: &str, organization: &str) -> Result<Membership, ApiError> {
        let role = account.strip_prefix("user-").unwrap_or("");
        if organization != "org-team" || !["admin", "member", "billing"].contains(&role) {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(Membership {
            id: format!("member-{role}"),
            user_id: account.into(),
            organization_id: organization.into(),
            status: "active".into(),
            role: MembershipRole { slug: role.into() },
        })
    }
}

async fn request(
    app: &Router,
    path: &str,
    token: &str,
    scope: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().uri(path);
    if !token.is_empty() {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(scope) = scope {
        builder = builder.header("x-zuse-workspace", scope);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn fixture() -> (PgPool, PgPool, String) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("Set explicit localhost ZUSE_CLOUD_TEST_DATABASE_URL");
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1")),
        "Tests require localhost"
    );
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("cloud_test_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let schema_copy = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let schema = schema_copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path = {schema}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("fixtures/legacy-catalog.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(jobs::MIGRATION).execute(&pool).await.unwrap();
    (admin, pool, schema)
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn catalog_authorization_archive_reads_and_durable_job_recovery() {
    let (admin, pool, schema) = fixture().await;
    let state = AppState {
        store: Store::from_pool(pool.clone()),
        identity: Arc::new(TestIdentity),
        organizations_enabled: true,
        organization_ids: Arc::new(vec!["org-team".into()]),
    };
    let app = router(state, vec![]);
    assert_eq!(request(&app, "/readyz", "", None).await.0, StatusCode::OK);
    assert_eq!(
        request(&app, "/v1/cloud/workspaces", "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, catalog) = request(&app, "/v1/cloud/workspaces", "alice", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["workspaces"][0]["workspaceId"], "ws-alice");
    for secret in [
        "PRIVATE",
        "request_config",
        "runtimeCredentialHash",
        "account_id",
        "machine-alice",
    ] {
        assert!(
            !catalog.to_string().contains(secret),
            "Internal field escaped public projection"
        );
    }
    assert_eq!(
        request(&app, "/v1/cloud/workspaces/ws-bob", "alice", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, archive) = request(
        &app,
        "/v1/cloud/workspaces/ws-alice/archive-status",
        "alice",
        None,
    )
    .await;
    assert_eq!(archive["retentionDeadline"], 100 + 10 * 86400000_i64);
    assert_eq!(archive["machineRetained"], true);
    assert_eq!(archive["backupVerified"], false);
    assert_eq!(archive["computeRetired"], false);
    let retained: String = sqlx::query_scalar(
        "SELECT provider_sandbox_id FROM api_cloud_workspaces WHERE workspace_id='ws-alice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retained, "machine-alice");
    let (_, entitlements) = request(&app, "/v1/billing/entitlements", "alice", None).await;
    assert_eq!(
        entitlements["entitlements"][0]["entitlementId"],
        "ent-alice"
    );
    assert!(!entitlements.to_string().contains("sub-preserved"));
    let org = Some("organization:org-team");
    assert_eq!(
        request(&app, "/v1/cloud/workspaces", "billing", org)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "/v1/billing/entitlements", "member", org)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "/v1/billing/entitlements", "billing", org)
            .await
            .0,
        StatusCode::OK
    );
    let (_, member_rows) = request(&app, "/v1/cloud/workspaces", "member", org).await;
    assert_eq!(member_rows["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(member_rows["workspaces"][0]["workspaceId"], "ws-org-shared");
    assert_eq!(
        request(&app, "/v1/cloud/workspaces/ws-org-private", "member", org)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "/v1/cloud/workspaces/ws-org-private", "admin", org)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/v1/cloud/workspaces", "alice", Some("organization:"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "/v1/cloud/workspaces",
            "admin",
            Some("organization:other")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    let mut tx = pool.begin().await.unwrap();
    let job_id = jobs::enqueue(
        &mut tx,
        "archive:one",
        "archive",
        "ws-alice",
        1,
        &json!({"operationId":"one"}),
        100,
    )
    .await
    .unwrap();
    assert_eq!(
        jobs::enqueue(
            &mut tx,
            "archive:one",
            "archive",
            "ws-alice",
            1,
            &json!({"operationId":"one"}),
            101
        )
        .await
        .unwrap(),
        job_id
    );
    assert!(
        jobs::enqueue(
            &mut tx,
            "archive:one",
            "archive",
            "ws-alice",
            1,
            &json!({"operationId":"other"}),
            101
        )
        .await
        .is_err()
    );
    tx.commit().await.unwrap();
    assert!(
        jobs::claim(&pool, "archive", "workspace", 100, 1000)
            .await
            .unwrap()
            .is_none(),
        "Legacy remains owner by default"
    );
    sqlx::query(
        "INSERT INTO zuse_rust_control_owners VALUES ('workspace','ws-alice','rust',1,100)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (first, second) = tokio::join!(
        jobs::claim(&pool, "archive", "workspace", 100, 1000),
        jobs::claim(&pool, "archive", "workspace", 100, 1000)
    );
    let mut claims = vec![first.unwrap(), second.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 1);
    let first = claims.pop().unwrap();
    let recovered = jobs::claim(&pool, "archive", "workspace", 1101, 1000)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.lease_token, recovered.lease_token);
    assert!(
        !jobs::finish(&pool, &first, "workspace", 1102)
            .await
            .unwrap()
    );
    assert!(
        jobs::retry(&pool, &recovered, 1102, "provider_unavailable")
            .await
            .unwrap()
    );
    assert!(
        jobs::claim(&pool, "archive", "workspace", 1103, 1000)
            .await
            .unwrap()
            .is_none()
    );
    let retry = jobs::claim(&pool, "archive", "workspace", 10000, 1000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        jobs::finish(&pool, &retry, "workspace", 10001)
            .await
            .unwrap()
    );
    assert!(
        jobs::claim(&pool, "archive", "workspace", 20000, 1000)
            .await
            .unwrap()
            .is_none()
    );

    // Exercise the actual production Store connection, not a parallel test implementation.
    let mut url =
        reqwest::Url::parse(&std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap()).unwrap();
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let readonly = Store::connect_read_only(url.as_str()).await.unwrap();
    let report = readonly.migration_report().await.unwrap();
    assert_eq!(report["databaseReadOnly"], true);
    assert_eq!(report["workspaces"]["workspaces"], 4);
    assert_eq!(report["ownershipTransferred"], false);
    drop(readonly);
    pool.close().await;
    // This schema name is generated above; no application or shared tables are dropped.
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
