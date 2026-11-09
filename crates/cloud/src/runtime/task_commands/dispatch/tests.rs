use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn lease_and_execution_binding_commit_together_and_roll_back_on_every_fence() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("dispatch_batch_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(2)
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
    sqlx::raw_sql(super::super::super::mailbox::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE zuse_rust_task_command_sessions(workspace_id text,session_id text,PRIMARY KEY(workspace_id,session_id)); INSERT INTO zuse_rust_task_command_sessions VALUES('ws','session')").execute(&pool).await.unwrap();
    let execution = super::super::MIGRATION
        .split("CREATE TABLE IF NOT EXISTS zuse_rust_task_command_execution")
        .nth(1)
        .unwrap()
        .split("CREATE INDEX IF NOT EXISTS zuse_rust_task_command_execution_lane")
        .next()
        .unwrap();
    sqlx::raw_sql(&format!(
        "CREATE TABLE zuse_rust_task_command_execution{execution}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql("INSERT INTO zuse_rust_mailboxes(workspace_id) VALUES('ws'); INSERT INTO zuse_rust_mailbox_commands(workspace_id,command_id,workspace_sequence,revision,lane,fingerprint,envelope,envelope_bytes,state,accepted_at,updated_at) VALUES('ws','command',1,0,'lane','fingerprint','{}',0,'accepted',1,1)").execute(&pool).await.unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(&[53; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([53;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([54; 32])).unwrap(),
        TicketSigner::from_jwk(&jwk.to_string(), "https://fixture.invalid").unwrap(),
    );
    for (state, generation, disk, allowed) in [
        ("queued", None, None, true),
        ("queued", Some(3), Some("disk"), true),
        ("queued", Some(2), Some("disk"), false),
        ("queued", Some(3), Some("other"), false),
        ("queued", Some(3), None, false),
        ("running", Some(3), Some("disk"), false),
        ("completed", Some(3), Some("disk"), false),
        ("missing", None, None, false),
    ] {
        sqlx::raw_sql("UPDATE zuse_rust_mailboxes SET revision=0; UPDATE zuse_rust_mailbox_commands SET state='accepted',ever_leased=false,lease_token=NULL,lease_deadline=NULL,runtime_generation=NULL,provider_sandbox_id=NULL,storage_incarnation_id=NULL,revision=0; DELETE FROM zuse_rust_task_command_execution").execute(&pool).await.unwrap();
        if state != "missing" {
            sqlx::query("INSERT INTO zuse_rust_task_command_execution(workspace_id,command_id,session_id,fingerprint,state,runtime_generation,storage_incarnation_id,presentation_ciphertext,created_at,updated_at) VALUES('ws','command','session','fingerprint',$1,$2,$3,'ciphertext',1,1)").bind(state).bind(generation).bind(disk).execute(&pool).await.unwrap();
        }
        let mut tx = pool.begin().await.unwrap();
        let result = control
            .task_lease_dispatch_tx(
                &mut tx, "ws", "command", "token", 200, 3, "machine", "disk", 100,
            )
            .await;
        if allowed {
            result.unwrap();
            tx.commit().await.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().code,
                "cloud_task_dispatch_execution_fenced"
            );
            tx.rollback().await.unwrap();
        }
        let row:Value=sqlx::query_scalar("SELECT jsonb_build_object('revision',m.revision,'commandRevision',c.revision,'state',c.state,'leased',c.ever_leased,'token',c.lease_token,'generation',c.runtime_generation,'sandbox',c.provider_sandbox_id,'disk',c.storage_incarnation_id) FROM zuse_rust_mailboxes m JOIN zuse_rust_mailbox_commands c USING(workspace_id)").fetch_one(&pool).await.unwrap();
        if allowed {
            assert_eq!(
                row,
                json!({"revision":1,"commandRevision":1,"state":"leased","leased":true,"token":"token","generation":3,"sandbox":"machine","disk":"disk"})
            );
            let binding:(i64,String)=sqlx::query_as("SELECT runtime_generation,storage_incarnation_id FROM zuse_rust_task_command_execution").fetch_one(&pool).await.unwrap();
            assert_eq!(binding, (3, "disk".into()));
        } else {
            assert_eq!(
                row,
                json!({"revision":0,"commandRevision":0,"state":"accepted","leased":false,"token":null,"generation":null,"sandbox":null,"disk":null})
            );
        }
    }
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
