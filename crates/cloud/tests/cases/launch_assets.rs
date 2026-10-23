use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use zuse_cloud::{crypto::MasterKey, runtime::RuntimeControl, tickets::TicketSigner};

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn launch_assets_round_trip_before_allocation_and_expired_upload_cleanup() {
    let (admin, pool, schema) = fixture().await;
    sqlx::raw_sql(include_str!("../fixtures/creation.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(zuse_cloud::runtime::assets::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    let at = chrono::Utc::now().timestamp_millis();
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,included_provider_cost_micros,markup_basis_points,overage_cap_micros) VALUES('period','user-bob','active',$1,$2,1000,500,500)").bind(at-1000).bind(at+86_400_000).execute(&pool).await.unwrap();
    let signing = SigningKey::from_bytes(&[26; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([26;32]),"x":B64.encode(signing.verifying_key().as_bytes())});
    let objects = Arc::new(ProjectionObjects::default());
    let control = Arc::new(
        RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&B64.encode([27; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api-rust.zuse.sh").unwrap(),
        )
        .with_creation_accounts(vec!["user-bob".into()])
        .with_transcript_objects(objects.clone()),
    );
    let app = router(
        AppState {
            store: Store::from_pool(pool.clone()),
            identity: Arc::new(TestIdentity),
            organizations_enabled: false,
            organization_ids: Arc::new(vec![]),
            runtime: Some(control.clone()),
            gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
        },
        vec![],
    );
    let bytes: Vec<u8> = (0..(512 * 1024 + 37)).map(|i| (i % 251) as u8).collect();
    let chunk = |seq: usize, part: &[u8]| json!({"idempotencyKey":"launch-assets-once","uploadId":"file-identity","seq":seq,"bytes":B64.encode(part)});
    let endpoint = "/v1/cloud/launch-assets/chunk";
    assert_eq!(
        runtime_post(&app, endpoint, "alice", &chunk(0, &bytes[..512 * 1024]))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, accepted) =
        runtime_post(&app, endpoint, "bob", &chunk(0, &bytes[..512 * 1024])).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    let workspace = accepted["workspaceId"].as_str().unwrap().to_owned();
    let asset_id = accepted["assetId"].as_str().unwrap().to_owned();
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &chunk(0, &bytes[..512 * 1024]))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &chunk(0, b"different"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let commit = json!({"idempotencyKey":"launch-assets-once","uploadId":"file-identity","originalName":"private-context.txt","mimeType":"text/plain","sizeBytes":bytes.len(),"sha256":B64.encode(Sha256::digest(&bytes))});
    let commit_endpoint = "/v1/cloud/launch-assets/commit";
    assert_eq!(
        runtime_post(&app, commit_endpoint, "bob", &commit).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &chunk(1, &bytes[512 * 1024..]))
            .await
            .0,
        StatusCode::OK
    );
    let (status, ready) = runtime_post(&app, commit_endpoint, "bob", &commit).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(
        runtime_post(&app, commit_endpoint, "bob", &commit).await.1,
        ready
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM api_cloud_workspaces WHERE workspace_id=$1")
            .bind(&workspace)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "Uploading cannot allocate or create a workspace");
    let db_manifest: Value =
        sqlx::query_scalar("SELECT manifest FROM zuse_rust_launch_assets WHERE workspace_id=$1")
            .bind(&workspace)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!db_manifest.to_string().contains("private-context"));
    assert!(
        objects
            .bytes
            .lock()
            .unwrap()
            .values()
            .all(|b| !String::from_utf8_lossy(b).contains("private-context"))
    );
    let body = json!({"projectId":"create-project","providerId":"boxd","baseRef":"main","agent":"codex","model":"fixture","firstMessage":"[Image 1](zuse-image:1) Read the launch attachment","idempotencyKey":"launch-assets-once","attachments":[ready["asset"].clone()]});
    let (status, error) = runtime_post(&app, "/v1/cloud/workspaces", "bob", &body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(
        error["error"],
        "cloud_launch_attachments_runtime_update_required"
    );
    sqlx::query("UPDATE api_cloud_project_builds SET settings=settings||'{\"rustLaunchAssetsVersion\":1}'::jsonb").execute(&pool).await.unwrap();
    let mut foreign = body.clone();
    foreign["attachments"][0]["originalName"] = json!("changed.txt");
    assert_eq!(
        runtime_post(&app, "/v1/cloud/workspaces", "bob", &foreign)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (status, created) = runtime_post(&app, "/v1/cloud/workspaces", "bob", &body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["workspace"]["workspaceId"], workspace);
    assert_eq!(
        created["workspace"]["title"],
        "Image 1 Read the launch attachment"
    );
    assert_eq!(
        runtime_post(&app, "/v1/cloud/workspaces", "bob", &body)
            .await
            .1["workspace"]["workspaceId"],
        workspace
    );
    let config = json!({"runtimeGeneration":1,"gatewayEpoch":1,"runtimeRpcProtocol":"zuse-rpc-v1","runtimeCredentialExpiresAtMs":at+60000,"runtimeBootstrapReceipt":{"workspaceId":workspace,"generation":1,"gatewayEpoch":1}});
    let credential = "private-runtime-asset-token";
    let hash = format!("{:x}", Sha256::digest(credential.as_bytes()));
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$2,request_config=request_config||$3 WHERE workspace_id=$1").bind(&workspace).bind(hash).bind(config).execute(&pool).await.unwrap();
    let path = format!("/v1/cloud/workspaces/{workspace}/runtime/attachments/{asset_id}");
    assert_eq!(
        request(&app, &path, "bob", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, download) = request(&app, &path, credential, None).await;
    assert_eq!(status, StatusCode::OK, "{download}");
    assert_eq!(download["asset"], ready["asset"]);
    assert_eq!(
        B64.decode(download["bytes"].as_str().unwrap()).unwrap(),
        bytes
    );
    let key = format!("workspaces/{workspace}/api-assets/{asset_id}/0.json");
    let original = objects
        .bytes
        .lock()
        .unwrap()
        .insert(key.clone(), b"corrupt".to_vec())
        .unwrap();
    assert!(!request(&app, &path, credential, None).await.0.is_success());
    objects.bytes.lock().unwrap().insert(key, original);
    // Original Node-produced v1 objects remain readable above the normal envelope limit.
    let legacy_id = "asset_legacy_original_identity";
    let script = r#"const c=require('node:crypto');const bytes=Buffer.alloc(6*1024*1024+3,73);const id=process.argv[1],ws=process.argv[2];const iv=Buffer.alloc(12,41),cipher=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,27),iv);cipher.setAAD(Buffer.from(`zuse-cloud-api-v1\nasset\nuser-bob\n${ws}\n${id}`));const stored={version:1,assetId:id,accountId:'user-bob',workspaceId:ws,mimeType:'application/zip',originalName:'legacy.zip',sizeBytes:bytes.length,sha256:c.createHash('sha256').update(bytes).digest('base64url'),bytes:bytes.toString('base64url'),createdAt:1};const ciphertext=Buffer.concat([cipher.update(JSON.stringify(stored)),cipher.final(),cipher.getAuthTag()]);process.stdout.write(JSON.stringify({version:1,iv:iv.toString('base64url'),ciphertext:ciphertext.toString('base64url')}));"#;
    let output = std::process::Command::new("node")
        .args(["-e", script, legacy_id, &workspace])
        .output()
        .unwrap();
    assert!(output.status.success());
    objects.bytes.lock().unwrap().insert(
        format!("workspaces/{workspace}/api-assets/{legacy_id}.json"),
        output.stdout,
    );
    let legacy = control
        .runtime_asset(&workspace, credential, legacy_id, at)
        .await
        .unwrap();
    assert_eq!(legacy.asset.size_bytes, 6 * 1024 * 1024 + 3);
    assert_eq!(
        B64.decode(&legacy.bytes).unwrap(),
        vec![73; 6 * 1024 * 1024 + 3]
    );
    // A different launch abandoned after upload loses only its own encrypted assets.
    let abandoned = json!({"idempotencyKey":"abandoned","uploadId":"abandoned-file","seq":0,"bytes":B64.encode(b"not accepted")});
    sqlx::raw_sql("CREATE FUNCTION fail_asset_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'asset database write fixture'; END $$; CREATE TRIGGER fail_asset_write BEFORE UPDATE ON zuse_rust_launch_assets FOR EACH ROW EXECUTE FUNCTION fail_asset_write();").execute(&pool).await.unwrap();
    assert!(
        !runtime_post(&app, endpoint, "bob", &abandoned)
            .await
            .0
            .is_success()
    );
    let durable_stage: String = sqlx::query_scalar(
        "SELECT workspace_id FROM zuse_rust_launch_staging WHERE workspace_id<>$1",
    )
    .bind(&workspace)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        objects
            .bytes
            .lock()
            .unwrap()
            .keys()
            .any(|key| key.starts_with(&format!("workspaces/{durable_stage}/")))
    );
    sqlx::raw_sql("DROP TRIGGER fail_asset_write ON zuse_rust_launch_assets; DROP FUNCTION fail_asset_write();").execute(&pool).await.unwrap();
    let (_, staged) = runtime_post(&app, endpoint, "bob", &abandoned).await;
    let abandoned_id = staged["workspaceId"].as_str().unwrap();
    assert_eq!(abandoned_id, durable_stage);
    sqlx::query("UPDATE zuse_rust_launch_staging SET expires_at=$1")
        .bind(at - 1)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        runtime_post(&app, endpoint, "bob", &abandoned).await.0,
        StatusCode::CONFLICT
    );
    assert!(control.sweep_launch_assets_once(at).await.unwrap());
    assert!(control.sweep_launch_assets_once(at).await.unwrap());
    assert!(!control.sweep_launch_assets_once(at).await.unwrap());
    assert!(
        !objects
            .bytes
            .lock()
            .unwrap()
            .keys()
            .any(|k| k.starts_with(&format!("workspaces/{abandoned_id}/")))
    );
    assert_eq!(
        request(&app, &path, credential, None).await.0,
        StatusCode::OK,
        "Bound workspace assets survive staging expiry"
    );
    sqlx::query("UPDATE api_cloud_workspaces SET state='deleting' WHERE workspace_id=$1")
        .bind(&workspace)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(&app, &path, credential, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
