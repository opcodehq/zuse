use super::*;
use crate::{
    auth::{Identity, Membership, MembershipRole, Principal},
    gateway::Hub,
    routes::{AppState, router},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::json;
use tower::ServiceExt;

struct TestIdentity;
struct VerifiedDriver;
struct RefreshDriver {
    calls: std::sync::atomic::AtomicUsize,
    entered: Option<Arc<tokio::sync::Notify>>,
    release: Option<Arc<tokio::sync::Notify>>,
}
#[async_trait::async_trait]
impl crate::connections::worker::AuthDriver for RefreshDriver {
    async fn execute(
        &self,
        job: crate::connections::worker::AuthJob,
        _: tokio::sync::mpsc::Sender<crate::connections::worker::DeviceChallenge>,
    ) -> Result<crate::connections::worker::VerifiedConnection, &'static str> {
        assert!(job.refresh);
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(entered) = &self.entered {
            entered.notify_one();
        }
        if let Some(release) = &self.release {
            release.notified().await;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        let expiry = chrono::Utc::now().timestamp_millis() + 3600000;
        Ok(crate::connections::worker::VerifiedConnection{credential:zeroize::Zeroizing::new(json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":"fresh-test-access","refreshToken":"rotated-test-refresh","providerAccountId":"provider-A","expiresAt":expiry,"nativeAuth":{}}).to_string()),account_label:None,account_id:Some("provider-A".into()),expires_at:Some(expiry)})
    }
}
#[async_trait::async_trait]
impl crate::connections::worker::AuthDriver for VerifiedDriver {
    async fn execute(
        &self,
        job: crate::connections::worker::AuthJob,
        _: tokio::sync::mpsc::Sender<crate::connections::worker::DeviceChallenge>,
    ) -> Result<crate::connections::worker::VerifiedConnection, &'static str> {
        assert_eq!(job.secret.as_str(), "test-private-key-value");
        Ok(crate::connections::worker::VerifiedConnection{credential:zeroize::Zeroizing::new(json!({"formatVersion":1,"agent":"codex","method":"api-key","secret":job.secret.as_str()}).to_string()),account_label:Some("Verified test connection".into()),account_id:None,expires_at:None})
    }
}
#[async_trait::async_trait]
impl Identity for TestIdentity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if token == "invalid" {
            return Err(ApiError::unauthorized("invalid_workos_token"));
        }
        Ok(Principal {
            account_id: format!("user-{token}"),
        })
    }
    async fn membership(&self, account: &str, organization: &str) -> Result<Membership, ApiError> {
        if organization != "org-team" {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(Membership {
            id: format!("member-{account}"),
            user_id: account.into(),
            organization_id: organization.into(),
            status: "active".into(),
            role: MembershipRole {
                slug: if account == "user-admin" {
                    "admin"
                } else {
                    "member"
                }
                .into(),
            },
        })
    }
}
async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    user: &str,
    org: bool,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {user}"))
        .header("content-type", "application/json");
    if org {
        request = request.header("x-zuse-workspace", "organization:org-team");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn environments_are_scoped_revisioned_and_available_without_compute_entitlement() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(url.contains("127.0.0.1") || url.contains("localhost"));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("computers_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |connection, _| {
            let schema = copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for sql in [
        include_str!("../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../tests/fixtures/projects.sql"),
        include_str!("../../migrations/0009_computers.sql"),
        // Connection refresh journals its rotations.
        include_str!("../../migrations/0024_credential_delivery_journal.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    sqlx::query("DELETE FROM api_entitlements WHERE account_id='user-alice'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO api_cloud_projects(project_id,account_id,repository_identity,repository_url,display_name,default_branch,visibility,git_connection_kind,configuration_digest,state,idempotency_key,created_at,updated_at) VALUES('project','user-alice','github.com/example/app','https://github.com/example/app.git','app','main','public','github-app','digest','connected','initial',0,0),('team-project','organization:org-team','github.com/team/app','https://github.com/team/app.git','team','main','private','github-app','digest','connected','team',0,0)").execute(&pool).await.unwrap();
    let store = Store::from_pool(pool.clone())
        .with_connections(crate::connections::ConnectionControl::from_pool(
            pool.clone(),
            crate::crypto::MasterKey::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            vec!["user-alice".into(), "organization:org-team".into()],
        ))
        .with_computer_control(ComputerControl::from_pool(
            pool.clone(),
            vec!["user-alice".into(), "organization:org-team".into()],
        ));
    let app = router(
        AppState {
            store,
            identity: Arc::new(TestIdentity),
            organizations_enabled: true,
            organization_ids: Arc::new(vec!["org-team".into()]),
            runtime: None,
            gateway: Arc::new(Hub::default()),
        },
        vec![],
    );
    let path = "/v1/cloud/projects/project/environment";
    assert_eq!(
        call(&app, "GET", path, "invalid", false, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", path, "bob", false, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    let initial = call(&app, "GET", path, "alice", false, Value::Null).await;
    assert_eq!(initial.0, StatusCode::OK);
    assert_eq!(initial.1["revision"], 0);
    let change = json!({"expectedRevision":0,"recipe":{"scripts":{"initialize":"npm ci"}}});
    let (a, b) = tokio::join!(
        call(&app, "PUT", path, "alice", false, change.clone()),
        call(&app, "PUT", path, "alice", false, change.clone())
    );
    assert!([a.0, b.0].contains(&StatusCode::OK));
    assert!([a.0, b.0].contains(&StatusCode::CONFLICT));
    let saved = call(&app, "GET", path, "alice", false, Value::Null).await.1;
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["recipe"]["scripts"]["initialize"], "npm ci");
    let secret = json!({"expectedRevision":1,"recipe":{"environment":{"API_KEY":"never-expose"}}});
    let rejected = call(&app, "PUT", path, "alice", false, secret).await;
    assert_eq!(rejected.0, StatusCode::BAD_REQUEST);
    assert!(!rejected.1.to_string().contains("never-expose"));
    assert_eq!(
        call(
            &app,
            "PUT",
            "/v1/cloud/projects/team-project/environment",
            "member",
            true,
            change.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            "/v1/cloud/projects/team-project/environment",
            "admin",
            true,
            change.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    sqlx::query(
        "UPDATE cloud_environment_versions SET source='repository' WHERE owner_id='user-alice'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut change = change;
    change["expectedRevision"] = json!(1);
    assert_eq!(
        call(&app, "PUT", path, "alice", false, change).await.1["error"],
        "environment_managed_in_repository"
    );
    let capabilities = call(
        &app,
        "GET",
        "/v1/cloud/capabilities",
        "alice",
        false,
        Value::Null,
    )
    .await;
    assert_eq!(capabilities.0, StatusCode::OK);
    assert_eq!(capabilities.1["providers"].as_array().unwrap().len(), 4);
    assert_eq!(capabilities.1["canAllocate"], false);
    assert_eq!(capabilities.1["canConfigure"], true);
    assert!(
        capabilities.1["allocationBlockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "cloud_entitlement_required")
    );
    assert!(
        capabilities.1["providers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["managedBaseReady"] == false)
    );
    // Saving credentials admits verification; it must not fabricate Connected.
    let credential = json!({"agent":"codex","method":"api-key","secret":"test-private-key-value","idempotencyKey":"configure-one"});
    let saved = call(
        &app,
        "POST",
        "/v1/cloud/agents/configure",
        "alice",
        false,
        credential.clone(),
    )
    .await;
    assert_eq!(saved.0, StatusCode::OK);
    assert_eq!(saved.1["state"], "pending");
    let connection = saved.1["connectionId"].as_str().unwrap();
    assert!(!saved.1.to_string().contains("test-private"));
    let retry = call(
        &app,
        "POST",
        "/v1/cloud/agents/configure",
        "alice",
        false,
        credential.clone(),
    )
    .await;
    assert_eq!(retry.1["connectionId"], connection);
    let encrypted: String = sqlx::query_scalar(
        "SELECT credential_ciphertext FROM cloud_agent_connections WHERE connection_id=$1",
    )
    .bind(connection)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!encrypted.contains("test-private-key-value"));
    let key =
        crate::crypto::MasterKey::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert_eq!(
        key.open_api(
            &format!("agent-connection:user-alice:{connection}"),
            &encrypted
        )
        .unwrap()
        .as_slice(),
        b"test-private-key-value"
    );
    assert!(
        key.open_api(
            &format!("agent-connection:user-bob:{connection}"),
            &encrypted
        )
        .is_err()
    );
    let authority = crate::connections::ConnectionControl::from_pool(
        pool.clone(),
        crate::crypto::MasterKey::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
        vec!["user-alice".into()],
    );
    assert!(authority.work_once(&VerifiedDriver).await.unwrap());
    assert!(!authority.work_once(&VerifiedDriver).await.unwrap());
    let verified = call(&app, "GET", "/v1/cloud/agents", "alice", false, Value::Null).await;
    assert_eq!(verified.1["connections"][0]["state"], "connected");
    assert_eq!(
        verified.1["connections"][0]["accountLabel"],
        "Verified test connection"
    );
    assert!(!verified.1.to_string().contains("test-private-key-value"));
    assert_eq!(
        authority
            .credential("user-alice", connection, "codex", false, &VerifiedDriver)
            .await
            .unwrap()
            .secret
            .as_str(),
        "test-private-key-value"
    );
    assert!(
        authority
            .credential("user-alice", connection, "grok", false, &VerifiedDriver)
            .await
            .is_err()
    );
    let now = chrono::Utc::now().timestamp_millis();
    let old=json!({"formatVersion":1,"agent":"codex","method":"device-code","accessToken":"old-test-access","refreshToken":"old-test-refresh","providerAccountId":"provider-A","expiresAt":now-1,"nativeAuth":{}}).to_string();
    for id in ["refresh-test", "revocation-test"] {
        let encrypted = key
            .seal_api(&format!("agent-connection:user-alice:{id}"), &old)
            .unwrap();
        sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,provider_account_id,credential_ciphertext,expires_at,authority_epoch,created_at,updated_at) VALUES($1,'user-alice',$1,'test','codex','device-code','connected','provider-A',$2,$3,9,$3,$3)").bind(id).bind(encrypted).bind(now-1).execute(&pool).await.unwrap();
    }
    let driver = RefreshDriver {
        calls: std::sync::atomic::AtomicUsize::new(0),
        entered: None,
        release: None,
    };
    let reads = futures::future::join_all(
        (0..10)
            .map(|_| authority.credential("user-alice", "refresh-test", "codex", false, &driver)),
    )
    .await;
    assert!(reads.iter().all(|r| {
        r.as_ref()
            .is_ok_and(|c| c.secret.as_str() == "fresh-test-access" && c.authority_epoch == 9)
    }));
    assert_eq!(driver.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let driver = RefreshDriver {
        calls: std::sync::atomic::AtomicUsize::new(0),
        entered: Some(entered.clone()),
        release: Some(release.clone()),
    };
    let refresh = authority.credential("user-alice", "revocation-test", "codex", false, &driver);
    let revoke = async {
        entered.notified().await;
        authority
            .disconnect(
                "user-alice",
                "revocation-test",
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        release.notify_one();
    };
    let (delivery, _) = tokio::join!(refresh, revoke);
    assert!(delivery.is_err());
    let state: String = sqlx::query_scalar(
        "SELECT state FROM cloud_agent_connections WHERE connection_id='revocation-test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "revoked");
    let mut changed = credential;
    changed["secret"] = json!("different-test-private-value");
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/cloud/agents/configure",
            "alice",
            false,
            changed
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "GET", "/v1/cloud/agents", "bob", false, Value::Null)
            .await
            .1["connections"],
        json!([])
    );
    let login = json!({"agent":"grok","connectionId":null,"idempotencyKey":"device-one"});
    let operation = call(
        &app,
        "POST",
        "/v1/cloud/agents/login",
        "alice",
        false,
        login.clone(),
    )
    .await;
    assert_eq!(operation.0, StatusCode::OK);
    assert_eq!(operation.1["state"], "queued");
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/cloud/agents/login",
            "alice",
            false,
            login
        )
        .await
        .1["operationId"],
        operation.1["operationId"]
    );
    let operation_path = format!(
        "/v1/cloud/agents/operations/{}",
        operation.1["operationId"].as_str().unwrap()
    );
    assert_eq!(
        call(&app, "GET", &operation_path, "bob", false, Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "DELETE", &operation_path, "alice", false, Value::Null)
            .await
            .1["state"],
        "cancelled"
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/v1/cloud/agents/{connection}"),
            "bob",
            false,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/v1/cloud/agents/{connection}"),
            "alice",
            false,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let removed: Option<String> = sqlx::query_scalar(
        "SELECT credential_ciphertext FROM cloud_agent_connections WHERE connection_id=$1",
    )
    .bind(connection)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(removed.is_none());
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn machine_recipes_are_revisioned_validated_and_report_image_state() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(url.contains("127.0.0.1") || url.contains("localhost"));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("machine_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let copy = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |connection, _| {
            let schema = copy.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={schema}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for sql in [
        include_str!("../../tests/fixtures/legacy-catalog.sql"),
        include_str!("../../tests/fixtures/projects.sql"),
        include_str!("../../migrations/0009_computers.sql"),
        include_str!("../../migrations/0031_machine_images.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    let store = Store::from_pool(pool.clone())
        .with_connections(crate::connections::ConnectionControl::from_pool(
            pool.clone(),
            crate::crypto::MasterKey::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap(),
            vec!["user-alice".into()],
        ))
        .with_computer_control(ComputerControl::from_pool(
            pool.clone(),
            vec!["user-alice".into()],
        ));
    let app = router(
        AppState {
            store,
            identity: Arc::new(TestIdentity),
            organizations_enabled: true,
            organization_ids: Arc::new(vec!["org-team".into()]),
            runtime: None,
            gateway: Arc::new(Hub::default()),
        },
        vec![],
    );
    let path = "/v1/cloud/machine";
    assert_eq!(
        call(&app, "GET", path, "invalid", false, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let initial = call(&app, "GET", path, "alice", false, Value::Null).await;
    assert_eq!(initial.0, StatusCode::OK);
    assert_eq!(initial.1["revision"], 0);
    assert_eq!(initial.1["imageState"], "none");
    let change = json!({"expectedRevision":0,"recipe":{"setupScript":"npm i -g pnpm","instructions":"Prefer pnpm."}});
    let (a, b) = tokio::join!(
        call(&app, "PUT", path, "alice", false, change.clone()),
        call(&app, "PUT", path, "alice", false, change.clone())
    );
    assert!([a.0, b.0].contains(&StatusCode::OK));
    assert!([a.0, b.0].contains(&StatusCode::CONFLICT));
    let saved = call(&app, "GET", path, "alice", false, Value::Null).await.1;
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["recipe"]["setupScript"], "npm i -g pnpm");
    // Nothing builds it yet, so a customized machine reports queued.
    assert_eq!(saved["imageState"], "queued");
    let credential = json!({"expectedRevision":1,"recipe":{"files":[{"path":".ssh/id_ed25519","content":"never"}]}});
    let rejected = call(&app, "PUT", path, "alice", false, credential).await;
    assert_eq!(rejected.0, StatusCode::BAD_REQUEST);
    assert!(!rejected.1.to_string().contains("never"));
    // Writes stay closed for accounts without computer setup enabled.
    let bob = call(
        &app,
        "PUT",
        path,
        "bob",
        false,
        json!({"expectedRevision":0,"recipe":{"setupScript":"x"}}),
    )
    .await;
    assert_eq!(bob.0, StatusCode::FORBIDDEN);
    let cleared = call(
        &app,
        "PUT",
        path,
        "alice",
        false,
        json!({"expectedRevision":1,"recipe":{}}),
    )
    .await;
    assert_eq!(cleared.0, StatusCode::OK);
    assert_eq!(cleared.1["imageState"], "none");
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
