use super::*;
use tokio::time::{Duration, timeout};

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn combined_workspace_read_preserves_owner_lock_order_and_fresh_generation() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("runtime_lock_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(6)
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
    sqlx::raw_sql("CREATE TABLE zuse_rust_control_owners(resource_kind text,resource_id text,backend text,generation bigint); CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,request_config jsonb); INSERT INTO zuse_rust_control_owners VALUES('workspace','ws','rust',1); INSERT INTO api_cloud_workspaces VALUES('ws','{\"runtimeGeneration\":1,\"gatewayEpoch\":2}');").execute(&pool).await.unwrap();
    for mode in [
        WorkspaceLock::Exclusive,
        WorkspaceLock::Command,
        WorkspaceLock::Artifact,
    ] {
        let mut tx = pool.begin().await.unwrap();
        let (_, generation, epoch) = locked_workspace_with(&mut tx, "ws", mode).await.unwrap();
        assert_eq!((generation, epoch), (1, 2));
        tx.commit().await.unwrap();
    }
    // Block the workspace, then verify that its waiting reader already owns
    // the generation fence. A reordered workspace -> owner lock would fail.
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT workspace_id FROM api_cloud_workspaces WHERE workspace_id='ws' FOR UPDATE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let (started, pid) = tokio::sync::oneshot::channel();
    let task_pool = pool.clone();
    let reader = tokio::spawn(async move {
        let mut tx = task_pool.begin().await.unwrap();
        let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        started.send(backend).unwrap();
        let result = locked_workspace(&mut tx, "ws").await.unwrap();
        tx.commit().await.unwrap();
        result
    });
    let pid = pid.await.unwrap();
    timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT coalesce(wait_event_type='Lock',false) FROM pg_stat_activity WHERE pid=$1",
            )
            .bind(pid)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let mut probe = pool.begin().await.unwrap();
    let error = sqlx::query(
        "SELECT generation FROM zuse_rust_control_owners WHERE resource_id='ws' FOR UPDATE NOWAIT",
    )
    .execute(&mut *probe)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    probe.rollback().await.unwrap();
    blocker.rollback().await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), reader)
            .await
            .unwrap()
            .unwrap()
            .1,
        1
    );

    // Rotate ownership and workspace metadata while a new reader is waiting
    // for ownership. It must return the matching updated workspace tuple.
    let mut change = pool.begin().await.unwrap();
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=2 WHERE resource_id='ws'")
        .execute(&mut *change)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config='{\"runtimeGeneration\":2,\"gatewayEpoch\":3}' WHERE workspace_id='ws'").execute(&mut *change).await.unwrap();
    let task_pool = pool.clone();
    let (started, pid) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        let mut tx = task_pool.begin().await.unwrap();
        let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        started.send(backend).unwrap();
        let result = locked_workspace(&mut tx, "ws").await;
        tx.commit().await.unwrap();
        result
    });
    let pid = pid.await.unwrap();
    timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT coalesce(wait_event_type='Lock',false) FROM pg_stat_activity WHERE pid=$1",
            )
            .bind(pid)
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    change.commit().await.unwrap();
    let (_, generation, epoch) = timeout(Duration::from_secs(3), reader)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!((generation, epoch), (2, 3));
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE zuse_rust_control_owners SET generation=3")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        locked_workspace(&mut tx, "ws").await.unwrap_err().code,
        "workspace_runtime_fenced"
    );
    tx.rollback().await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
