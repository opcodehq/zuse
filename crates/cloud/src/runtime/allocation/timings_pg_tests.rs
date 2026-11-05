use super::*;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn defer_observations_wait_for_workspace_before_locking_job_and_preserve_config() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated database required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let schema = format!("startup_timing_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(4)
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
    sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,request_config jsonb NOT NULL);CREATE TABLE zuse_rust_jobs(job_id text PRIMARY KEY,resource_id text,resource_generation bigint,state text,lease_token text,lease_expires_at bigint,due_at bigint,updated_at bigint);INSERT INTO api_cloud_workspaces VALUES('workspace','{\"runtimeGeneration\":1,\"rustAllocationOperationId\":\"operation\",\"untouched\":true,\"startupTimings\":{\"enrolledAt\":7}}');INSERT INTO zuse_rust_jobs VALUES('job','workspace',1,'running','lease',9999,0,0)").execute(&pool).await.unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query(
        "SELECT workspace_id FROM api_cloud_workspaces WHERE workspace_id='workspace' FOR UPDATE",
    )
    .execute(&mut *held)
    .await
    .unwrap();
    let mut worker = pool.acquire().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *worker)
        .await
        .unwrap();
    let task = tokio::spawn(async move {
        sqlx::query(DEFER_ALLOCATION_SQL)
            .bind("job")
            .bind("lease")
            .bind(200i64)
            .bind(100i64)
            .bind(json!({"providerInspect":{"lastDurationMs":3}}))
            .bind("operation")
            .bind("workspace")
            .execute(&mut *worker)
            .await
    });
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock')",
        )
        .bind(pid)
        .fetch_one(&admin)
        .await
        .unwrap();
        if waiting {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "worker must reach the actual held workspace lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // If the telemetry CTE locks the job first this fails with lock_not_available.
    let mut inspector = pool.begin().await.unwrap();
    sqlx::query("SELECT job_id FROM zuse_rust_jobs WHERE job_id='job' FOR UPDATE NOWAIT")
        .execute(&mut *inspector)
        .await
        .unwrap();
    inspector.commit().await.unwrap();
    held.commit().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (config,state):(Value,String)=sqlx::query_as("SELECT request_config,(SELECT state FROM zuse_rust_jobs WHERE job_id='job')FROM api_cloud_workspaces WHERE workspace_id='workspace'").fetch_one(&pool).await.unwrap();
    assert_eq!(state, "pending");
    assert_eq!(config["untouched"], true);
    assert_eq!(config["startupTimings"]["enrolledAt"], 7);
    assert_eq!(
        config["startupTimings"]["providerPhases"]["providerInspect"]["lastDurationMs"],
        3
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
