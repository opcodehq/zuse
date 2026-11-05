use super::*;
use tokio::time::{Duration, timeout};

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn transport_preserves_snapshots_binding_rollback_and_invoker_privileges() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let schema = format!("runtime_batch_{suffix}");
    let role = format!("runtime_batch_role_{suffix}");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE ROLE {role}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
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
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    sqlx::raw_sql("CREATE TABLE sample(n bigint); INSERT INTO sample VALUES(1); CREATE TABLE protected(n bigint); INSERT INTO protected VALUES(99);").execute(&pool).await.unwrap();
    let literal = "x'); DROP TABLE sample; --";
    let mut tx = pool.begin().await.unwrap();
    let result = run(
        &mut tx,
        &[
            ("first", "SELECT to_jsonb($1->>'literal')"),
            (
                "second",
                "SELECT jsonb_build_object('prior',$2->'first','value',n) FROM sample",
            ),
        ],
        json!({"literal":literal}),
    )
    .await
    .unwrap();
    assert_eq!(result["second"], json!({"prior":literal,"value":1}));
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let error = run(&mut tx, &[
        ("changed", "WITH changed AS(UPDATE sample SET n=3 RETURNING n) SELECT to_jsonb(n) FROM changed"),
        ("failed", "SELECT to_jsonb(1/0)"),
    ], json!({})).await.unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    tx.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT n FROM sample")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );

    // The second statement must take a new snapshot after the first waits on a
    // lock, rather than reusing the outer SELECT's old snapshot.
    let mut change = pool.begin().await.unwrap();
    sqlx::query("UPDATE sample SET n=2")
        .execute(&mut *change)
        .await
        .unwrap();
    let (started, pid) = tokio::sync::oneshot::channel();
    let task_pool = pool.clone();
    let reader = tokio::spawn(async move {
        let mut tx = task_pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        started.send(pid).unwrap();
        let result = run(
            &mut tx,
            &[
                ("locked", "SELECT to_jsonb(n) FROM sample FOR UPDATE"),
                ("fresh", "SELECT to_jsonb(n) FROM sample"),
            ],
            json!({}),
        )
        .await
        .unwrap();
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
    let result = timeout(Duration::from_secs(3), reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, json!({"locked":2,"fresh":2}));

    sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO {role}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(&format!("GRANT SELECT,UPDATE ON sample TO {role}"))
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {role}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        run(
            &mut tx,
            &[("read", "SELECT to_jsonb(n) FROM sample")],
            json!({})
        )
        .await
        .is_err(),
        "PUBLIC must have no execute privilege"
    );
    tx.rollback().await.unwrap();
    sqlx::query(&format!(
        "GRANT EXECUTE ON FUNCTION zuse_rust_runtime_statement_batch(jsonb,jsonb) TO {role}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {role}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        run(
            &mut tx,
            &[("read", "SELECT to_jsonb(n) FROM sample")],
            json!({})
        )
        .await
        .unwrap()["read"],
        2
    );
    assert!(
        run(
            &mut tx,
            &[("read", "SELECT to_jsonb(n) FROM protected")],
            json!({})
        )
        .await
        .is_err(),
        "batch must not acquire the function owner's table privileges"
    );
    tx.rollback().await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {role}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn candidate_batches_preserve_registry_and_capacity_checks_without_cached_authorization() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("candidate_batch_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(3)
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
    fixture_schema(&pool).await;
    sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,provider text,state text,request_config jsonb)").execute(&pool).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let pin = crate::managed_bases::candidate::fixture_registered_pin(&pool, "user-bob", now)
        .await
        .unwrap();
    let manifest: Value = sqlx::query_scalar(
        "SELECT candidate_manifest FROM cloud_managed_bases WHERE release_id=$1",
    )
    .bind(&pin.release_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let candidate: crate::managed_bases::candidate::ManagedBaseCandidate =
        serde_json::from_value(manifest.clone()).unwrap();
    let mut config = candidate.settings(&pin);
    config["rustCreateRequestDigest"] = json!("fixture");
    let row = json!({"workspace_id":"ws","account_id":"user-bob","provider":"boxd","state":"setup","request_config":config});
    sqlx::query("INSERT INTO api_cloud_workspaces VALUES('ws','user-bob','boxd','setup',$1)")
        .bind(&row["request_config"])
        .execute(&pool)
        .await
        .unwrap();
    for damage in ["none", "state", "artifact", "candidate", "inventory"] {
        sqlx::query("UPDATE cloud_managed_bases SET state='preparing',artifact_id=$2,candidate_manifest=$3 WHERE release_id=$1").bind(&pin.release_id).bind(&pin.artifact_id).bind(&manifest).execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM api_cloud_workspaces WHERE workspace_id<>'ws'")
            .execute(&pool)
            .await
            .unwrap();
        match damage {
            "state" => {
                sqlx::query("UPDATE cloud_managed_bases SET state='revoked'")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "artifact" => {
                sqlx::query("UPDATE cloud_managed_bases SET artifact_id='different'")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "candidate" => {
                sqlx::query("UPDATE cloud_managed_bases SET candidate_manifest=jsonb_set(candidate_manifest,'{runtimeSha256}',to_jsonb(repeat('0',64)))").execute(&pool).await.unwrap();
            }
            "inventory" => {
                sqlx::query("INSERT INTO api_cloud_workspaces SELECT x,'user-bob','boxd','setup',$1 FROM unnest(ARRAY['other-one','other-two'])x").bind(&row["request_config"]).execute(&pool).await.unwrap();
            }
            _ => {}
        }
        let mut tx = pool.begin().await.unwrap();
        let observed = run(
            &mut tx,
            CAPACITY_READ,
            candidate_arguments(&pin, &row).unwrap(),
        )
        .await
        .unwrap();
        let actual = pin
            .validate_capacity_observation(&row, &observed, now)
            .map_err(|e| e.code);
        let expected =
            crate::managed_bases::candidate::compute_allowance(&mut tx, Some(&pin), &row, now)
                .await
                .map_err(|e| e.code);
        assert_eq!(actual, expected, "{damage}");
        assert_eq!(actual.is_ok(), damage == "none", "{damage}");
        tx.rollback().await.unwrap();
    }
    // Scope eligibility must still re-read a revocation on its next call.
    sqlx::query("UPDATE cloud_managed_bases SET state='preparing',artifact_id=$2,candidate_manifest=$3 WHERE release_id=$1").bind(&pin.release_id).bind(&pin.artifact_id).bind(&manifest).execute(&pool).await.unwrap();
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let signing = ed25519_dalek::SigningKey::from_bytes(&[74; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([74;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([75; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
    )
    .with_staging_base_pin(Some(pin))
    .with_fast_start_accounts(vec!["user-bob".into()]);
    control.candidate_row_allowed(&row, now).await.unwrap();
    sqlx::query("UPDATE cloud_managed_bases SET state='revoked'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(control.candidate_row_allowed(&row, now).await.is_err());
    drop(control);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
