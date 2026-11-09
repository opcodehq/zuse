//! Real SQL admission fences paired with a public-reference HTTP fixture.
use super::*;
pub(super) async fn exercise(
    pool: &PgPool,
    runtime: &crate::runtime::RuntimeControl,
    identity: &CacheIdentity,
    recipe: &Value,
    cache: &Value,
) {
    sqlx::raw_sql("CREATE TABLE zuse_rust_jobs(job_id text,resource_id text,resource_generation bigint,lease_token text,state text,lease_expires_at bigint); CREATE TABLE zuse_rust_control_owners(resource_id text,resource_kind text,backend text,generation bigint); INSERT INTO zuse_rust_control_owners VALUES('ws-alice','workspace','rust',1); INSERT INTO zuse_rust_jobs VALUES('ref-job','ws-alice',1,'ref-lease','running',9999999999999); UPDATE zuse_rust_allocations SET phase='planned',snapshot_id='managed-artifact'; UPDATE cloud_project_cache_capabilities SET state='certified';").execute(pool).await.unwrap();
    let config = json!({"runtimeGeneration":1,"rustActorId":"user-alice","rustAllocationOperationId":"allocation","managedReleaseId":"base-v1","environmentRecipe":recipe,"environmentRevision":1,"rustRepositoryCheckout":{"baseRef":"main","fetchRemote":true}});
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,desired_state='ready' WHERE workspace_id='ws-alice'").bind(config).execute(pool).await.unwrap();
    let commit = identity.source_commit.clone();
    let replies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls = replies.clone();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let commit = commit.clone();
        let calls = calls.clone();
        async move {
            calls.lock().unwrap().push(uri.to_string());
            if uri.path().contains("/contents/") {
                return (axum::http::StatusCode::NOT_FOUND, axum::Json(json!({})));
            }
            let value = if uri.path().contains("/commits/") {
                json!({"sha":commit})
            } else {
                json!({"id":77,"private":false,"full_name":"example/app"})
            };
            (axum::http::StatusCode::OK, axum::Json(value))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let job = crate::jobs::Job {
        id: "ref-job".into(),
        kind: "workspace-allocate".into(),
        resource_id: "ws-alice".into(),
        generation: 1,
        payload: json!({"operationId":"allocation"}),
        lease_token: "ref-lease".into(),
        attempts: 1,
    };
    let mut workspace: Value = sqlx::query_scalar(
        "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id='ws-alice'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let mut operation: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(
        runtime
            .resolve_project_allocation_source_at(&job, &mut workspace, &mut operation, &origin)
            .await
            .unwrap()
    );
    assert_eq!(operation["snapshot_id"], "prepared-artifact");
    assert_eq!(
        workspace["request_config"]["rustResolvedSource"]["commit"],
        identity.source_commit
    );
    assert_eq!(workspace["base_ref"], "main");
    assert_eq!(
        workspace["request_config"]["rustRepositoryCheckout"]["baseRef"],
        identity.source_commit
    );
    // Retry resolves the pinned SHA, never a moved main. No duplicate request.
    assert!(
        !runtime
            .resolve_project_allocation_source_at(&job, &mut workspace, &mut operation, &origin)
            .await
            .unwrap()
    );
    let refs = replies.lock().unwrap().clone();
    assert!(refs[1].ends_with("/commits/main"));
    assert!(refs[5].ends_with(&format!("/commits/{}", identity.source_commit)));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM zuse_rust_project_preparation_requests WHERE requested_ref=$1",
    )
    .bind(&identity.source_commit)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    // Lease withdrawal while public metadata is being read cannot change source.
    sqlx::query("UPDATE zuse_rust_jobs SET lease_token='new-lease'")
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .resolve_project_allocation_source_at(&job, &mut workspace, &mut operation, &origin)
            .await
            .unwrap_err()
            .code,
        "project_cache_allocation_fenced"
    );
    // Once provider creation may have happened, even a changed ref is ignored.
    operation["phase"] = json!("creating");
    let before = replies.lock().unwrap().len();
    assert!(
        !runtime
            .resolve_project_allocation_source_at(&job, &mut workspace, &mut operation, &origin)
            .await
            .unwrap()
    );
    assert_eq!(before, replies.lock().unwrap().len());
    server.abort();
    sqlx::raw_sql("DROP TABLE zuse_rust_jobs,zuse_rust_control_owners; UPDATE cloud_project_cache_capabilities SET state='retired'; UPDATE zuse_rust_allocations SET phase='creating',snapshot_id='prepared-artifact';").execute(pool).await.unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-alice'").bind(json!({"runtimeGeneration":1,"rustProjectCache":cache,"repositoryCache":"project-environment-v1"})).execute(pool).await.unwrap();
}
