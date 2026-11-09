//! Existing immutable request fixtures exercise the actual API credential authority.
use super::*;
pub(super) async fn exercise(
    pool: &PgPool,
    runtime: &crate::runtime::RuntimeControl,
    signer: &crate::tickets::TicketSigner,
    request: &str,
) {
    sqlx::raw_sql(include_str!("../../migrations/0015_github.sql"))
        .execute(pool)
        .await
        .unwrap();
    let fixture = crate::github::native::admission_fixture::connected_for_preparation(
        pool,
        "user-alice",
        "user-alice",
    )
    .await;
    runtime
        .configure_scope_authority(
            Arc::new(crate::github::native::admission_fixture::PersonalIdentity),
            Some(fixture.github.clone()),
            vec![],
        )
        .unwrap();
    let original_project: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM api_cloud_projects p WHERE project_id='project'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE api_cloud_projects SET visibility='private',repository_identity='github.com/example/repo',repository_url='https://github.com/example/repo.git' WHERE project_id='project'").execute(pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_project_preparation_requests SET repository_identity='github.com/example/repo' WHERE request_id=$1").bind(request).execute(pool).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let proof = signer
        .allocation_scope_proof(&format!("preparation:{request}"), "project:project", 1, now)
        .unwrap();
    let grant = runtime
        .project_preparation_source_credential("project", request, &proof, now)
        .await
        .unwrap();
    assert_eq!(grant["githubUserId"], 42);
    assert_eq!(grant["expiresAt"], grant["authorityExpiresAt"]);
    assert!(
        grant["providerExpiresAt"].as_i64().unwrap()
            >= grant["authorityExpiresAt"].as_i64().unwrap()
    );
    assert_eq!(grant["token"], "synthetic-scoped-preparation-token");
    let source_proof = grant["authorization"].as_str().unwrap();
    assert!(!source_proof.contains("synthetic"));
    runtime
        .revalidate_project_source("project", request, &proof, now, 42, source_proof)
        .await
        .unwrap();
    assert!(
        runtime
            .revalidate_project_source("project", request, &proof, now, 43, source_proof)
            .await
            .is_err()
    );
    assert!(
        runtime
            .revalidate_project_source("project", request, &proof, now, 42, "tampered")
            .await
            .is_err()
    );
    // A new authorization can succeed after relink while the ORIGINAL fetch's
    // epoch is invalid. The returned sealed proof must reject that source.
    sqlx::query("UPDATE zuse_rust_github_scopes SET epoch=epoch+1 WHERE owner_id='user-alice'")
        .execute(pool)
        .await
        .unwrap();
    runtime
        .authorize_project_preparation("project", request, &proof, now)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .revalidate_project_source("project", request, &proof, now, 42, source_proof)
            .await
            .unwrap_err()
            .code,
        "github_repository_access_denied"
    );
    sqlx::query("UPDATE zuse_rust_project_preparation_requests SET actor_id='foreign-actor' WHERE request_id=$1").bind(request).execute(pool).await.unwrap();
    assert_eq!(
        runtime
            .project_preparation_source_credential("project", request, &proof, now)
            .await
            .unwrap_err()
            .code,
        "project_preparation_scope_denied"
    );
    sqlx::query("UPDATE zuse_rust_project_preparation_requests SET actor_id='user-alice' WHERE request_id=$1").bind(request).execute(pool).await.unwrap();
    let persisted: String = sqlx::query_scalar("SELECT to_jsonb(r)::text FROM zuse_rust_project_preparation_requests r WHERE request_id=$1").bind(request).fetch_one(pool).await.unwrap();
    assert!(!persisted.contains("synthetic-scoped-preparation-token"));
    assert!(!persisted.contains(source_proof));
    // Retained tasks cannot use this endpoint to resolve or fetch a branch again.
    let original_config: Value = sqlx::query_scalar(
        "SELECT request_config FROM api_cloud_workspaces WHERE workspace_id='ws-alice'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let mut config = original_config.clone();
    config["rustGithubUserId"] = json!(42);
    config["rustActorId"] = json!("user-alice");
    config["rustAllocationOperationId"] = json!("allocation");
    config["environmentRecipe"] =
        json!(zuse_protocol::cloud_computers::EnvironmentRecipe::default());
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-alice'")
        .bind(&config)
        .execute(pool)
        .await
        .unwrap();
    let allocation_proof = signer
        .allocation_scope_proof("ws-alice", "allocation", 1, now)
        .unwrap();
    // This fixture's operation is restored to planned only for this first-import check.
    sqlx::query("UPDATE zuse_rust_allocations SET phase='planned',machine_id=NULL")
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .private_project_allocation_source_at(
                "ws-alice",
                &allocation_proof,
                now,
                &fixture.origin
            )
            .await
            .unwrap_err()
            .code,
        "project_cache_allocation_fenced"
    );
    let original_state: String =
        sqlx::query_scalar("SELECT state FROM api_cloud_workspaces WHERE workspace_id='ws-alice'")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        "UPDATE api_cloud_workspaces SET state='provisioning' WHERE workspace_id='ws-alice'",
    )
    .execute(pool)
    .await
    .unwrap();
    let source = runtime
        .private_project_allocation_source_at("ws-alice", &allocation_proof, now, &fixture.origin)
        .await
        .unwrap();
    assert_eq!(source["commit"], "c".repeat(40));
    assert_eq!(source["recipeSource"], "project");
    assert!(!source.to_string().contains("token"));
    config["rustGithubUserId"] = json!(43);
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-alice'")
        .bind(&config)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .private_project_allocation_source_at(
                "ws-alice",
                &allocation_proof,
                now,
                &fixture.origin
            )
            .await
            .unwrap_err()
            .code,
        "github_account_changed"
    );
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1 WHERE workspace_id='ws-alice'")
        .bind(original_config)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET state=$1 WHERE workspace_id='ws-alice'")
        .bind(original_state)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE zuse_rust_allocations SET phase='creating'")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_projects SET visibility=$1,repository_identity=$2,repository_url=$3 WHERE project_id='project'").bind(original_project["visibility"].as_str()).bind(original_project["repository_identity"].as_str()).bind(original_project["repository_url"].as_str()).execute(pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_project_preparation_requests SET repository_identity=$2 WHERE request_id=$1").bind(request).bind(original_project["repository_identity"].as_str()).execute(pool).await.unwrap();
}
use std::sync::Arc;
