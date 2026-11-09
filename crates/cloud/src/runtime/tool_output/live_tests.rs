//! Run only on Hetzner with private staging R2 env and a reverse-tunneled isolated
//! localhost PostgreSQL cluster. No provider or existing workspace is contacted.
use super::*;
fn api(error: crate::error::ApiError) -> anyhow::Error {
    anyhow::anyhow!(error.code)
}
use crate::{access::Scope, auth::Principal};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
#[tokio::test]
#[ignore = "requires private staging R2 and explicit isolated localhost PostgreSQL"]
async fn real_r2_large_tool_sidecar_is_encrypted_replayable_scoped_and_readable_asleep()
-> anyhow::Result<()> {
    // Only against the explicit isolated staging R2 fixture; elsewhere
    // (`--include-ignored` in CI) there is nothing to exercise.
    if std::env::var("ZUSE_CLOUD_TEST_REAL_R2").as_deref() != Ok("true")
        || std::env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() != Ok("staging")
    {
        return Ok(());
    }
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")?;
    anyhow::ensure!(
        matches!(
            reqwest::Url::parse(&url)?.host_str(),
            Some("127.0.0.1" | "localhost" | "::1")
        ),
        "Isolated localhost database required"
    );
    let objects = crate::objects::R2::from_env()?
        .ok_or_else(|| anyhow::anyhow!("Private staging object store required"))?;
    let admin = PgPool::connect(&url).await?;
    let schema = format!("tool_sidecar_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await?;
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(3)
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
        .await?;
    let id = format!("isolated-sidecar-{}", uuid::Uuid::new_v4());
    let prefix = format!("workspaces/{}/", crate::transcript::component(&id));
    println!("isolated R2 sidecar prefix={prefix}");
    let began = std::time::Instant::now();
    let result=async {
        sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,request_config jsonb,state text,desired_state text,runtime_credential_hash text);CREATE TABLE zuse_rust_control_owners(resource_kind text,resource_id text,backend text,generation bigint)").execute(&pool).await?;
        sqlx::raw_sql(include_str!("../../../migrations/0016_task_history.sql")).execute(&pool).await?;
        let key=B64.encode([81;32]);let signing=ed25519_dalek::SigningKey::from_bytes(&[82;32]);let signer=TicketSigner::from_jwk(&json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([82;32]),"x":B64.encode(signing.verifying_key().as_bytes())}).to_string(),"https://fixture.example").map_err(api)?;
        let control=RuntimeControl::from_pool(pool.clone(),MasterKey::parse(&key).map_err(|_|anyhow::anyhow!("fixture key invalid"))?,signer).with_transcript_objects(objects.clone());
        let now=chrono::Utc::now().timestamp_millis();let config=json!({"runtimeGeneration":1,"gatewayEpoch":1,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeBootstrapReceipt":{"workspaceId":id,"generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+3600000});
        sqlx::query("INSERT INTO api_cloud_workspaces VALUES($1,'fixture-owner',$2,'ready','ready',$3)").bind(&id).bind(config).bind(hash("isolated-runtime-credential")).execute(&pool).await?;
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace',$1,'rust',1)").bind(&id).execute(&pool).await?;
        let reference="fixture-session/tool-output";
        sqlx::query("INSERT INTO zuse_rust_tool_output_refs VALUES($1,$2,'fixture-session')").bind(&id).bind(reference).execute(&pool).await?;
        let text="live-r2-private-tool-output-".repeat(12000);let mut offset=0usize;let mut chunks=Vec::new();
        while offset<text.len(){let next=(offset+CHUNK_BYTES).min(text.len());let chunk=ToolOutputChunk{blob_ref:reference.into(),offset,next_offset:next,total_bytes:text.len(),text:text[offset..next].into()};
            control.tool_output_upload(&id,"isolated-runtime-credential",chunk.clone(),now).await.map_err(api)?;
            control.tool_output_upload(&id,"isolated-runtime-credential",chunk.clone(),now).await.map_err(api)?;
            chunks.push(chunk);offset=next;
        }
        let stored:Vec<(Option<String>,String,bool)>=sqlx::query_as("SELECT ciphertext,object_key,ready FROM zuse_rust_tool_output_chunks WHERE workspace_id=$1 ORDER BY byte_offset").bind(&id).fetch_all(&pool).await?;
        anyhow::ensure!(stored.len()==chunks.len() && stored.iter().all(|(cipher,_,ready)|cipher.is_none()&&*ready),"large output must be staged in immutable objects, not duplicated in PG");
        for (_,key,_) in &stored {let bytes=objects.get(key).await.map_err(api)?.ok_or_else(||anyhow::anyhow!("Missing immutable tool object"))?;anyhow::ensure!(!String::from_utf8_lossy(&bytes).contains("live-r2-private-tool-output"),"Object contains plaintext tool output");}
        let mut changed=chunks[0].clone();changed.text.replace_range(..1,"X");anyhow::ensure!(matches!(control.tool_output_upload(&id,"isolated-runtime-credential",changed,now).await,Err(error) if error.code=="tool_output_immutable_conflict"),"Conflicting replay accepted");
        sqlx::query("UPDATE api_cloud_workspaces SET state='paused',desired_state='paused' WHERE workspace_id=$1").bind(&id).execute(&pool).await?;
        let access=Access{actor:Principal{account_id:"fixture-owner".into()},scope:Scope::Personal,owner_id:"fixture-owner".into(),membership:None};
        let stranger=Access{actor:Principal{account_id:"stranger".into()},scope:Scope::Personal,owner_id:"stranger".into(),membership:None};
        anyhow::ensure!(matches!(control.tool_output_read(&stranger,&id,reference,0,CHUNK_BYTES).await,Err(error) if error.code=="cloud_workspace_not_found"),"Cross-owner output read accepted");
        let mut restored=String::new();for chunk in &chunks {let page=control.tool_output_read(&access,&id,reference,chunk.offset,CHUNK_BYTES).await.map_err(api)?;restored.push_str(page["text"].as_str().ok_or_else(||anyhow::anyhow!("Missing output"))?);anyhow::ensure!(page["nextOffset"]==chunk.next_offset,"Tool cursor changed");}
        anyhow::ensure!(restored==text,"Restored output differs");
        let state:String=sqlx::query_scalar("SELECT state FROM api_cloud_workspaces WHERE workspace_id=$1").bind(&id).fetch_one(&pool).await?;anyhow::ensure!(state=="paused","History access woke compute");
        println!("real R2 sidecar bytes={} chunks={} elapsedMs={} sleepingRead=true scopeRejected=true immutableReplay=true",text.len(),chunks.len(),began.elapsed().as_millis());
        Ok::<(),anyhow::Error>(())
    }.await;
    // Always reconcile only the random fixture prefix, even after uncertain puts.
    let cleanup = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        for _ in 0..12 {
            if objects.purge_prefix(&prefix).await? {
                return Ok::<bool, crate::error::ApiError>(true);
            }
        }
        Ok(false)
    })
    .await;
    pool.close().await;
    let schema_cleanup = sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await;
    admin.close().await;
    println!(
        "isolated R2 sidecar cleanupVerified={}",
        matches!(cleanup, Ok(Ok(true)))
    );
    anyhow::ensure!(
        matches!(cleanup, Ok(Ok(true))),
        "Fixture prefix cleanup requires reconciliation: {prefix}"
    );
    schema_cleanup?;
    result
}
