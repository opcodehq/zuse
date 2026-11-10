//! Real HTTP/WebSocket + isolated Postgres acceptance, no provider runtime.
use super::*;
use crate::events::hub::{Hub as EventHub, PROTOCOL};
use crate::{
    auth::{Identity, Membership, MembershipRole, Principal},
    crypto::MasterKey,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{SinkExt, StreamExt};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_tungstenite::tungstenite::{Message as ClientMessage, client::IntoClientRequest};
struct Member {
    active: AtomicBool,
}
#[async_trait::async_trait]
impl Identity for Member {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if token != "test-actor-token" {
            return Err(ApiError::unauthorized("test_token_rejected"));
        }
        Ok(Principal {
            account_id: "actor".into(),
        })
    }
    async fn membership(&self, actor: &str, org: &str) -> Result<Membership, ApiError> {
        if !self.active.load(Ordering::SeqCst) || actor != "actor" || org != "team" {
            return Err(ApiError::forbidden("test_member_departed"));
        }
        Ok(Membership {
            id: "member".into(),
            user_id: actor.into(),
            organization_id: org.into(),
            status: "active".into(),
            role: MembershipRole {
                slug: "member".into(),
            },
        })
    }
}
fn policy(creator: &str) -> Value {
    json!({"audience":"private","permission":"view","creatorSubject":creator,"creatorMembershipId":if creator=="actor"{"member"}else{"other"},"grants":[],"revision":1})
}
async fn frame<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(4), socket.next())
            .await
            .expect("bounded live socket wait")
            .expect("socket open")
            .expect("valid socket message");
        if let ClientMessage::Text(text) = message {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "ready" {
                socket
                    .send(ClientMessage::Text(
                        json!({"type":"ack","cursor":value["cursor"]})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                continue;
            }
            return value;
        }
    }
}
async fn new_ticket(
    http: &reqwest::Client,
    origin: &str,
    device: uuid::Uuid,
    cursor: Option<&str>,
) -> String {
    let response = http
        .post(format!("{origin}/v1/cloud/events/ticket"))
        .bearer_auth("test-actor-token")
        .header("x-zuse-workspace", "organization:team")
        .json(&json!({"deviceId":device,"cursor":cursor}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json::<Value>().await.unwrap()["ticket"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn socket_request(origin: &str, ticket: &str) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = origin
        .replace("http://", "ws://")
        .into_client_request()
        .unwrap();
    *request.uri_mut() = format!("{}/v1/cloud/events", origin.replace("http://", "ws://"))
        .parse()
        .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("{PROTOCOL}, auth.{ticket}").parse().unwrap(),
    );
    request
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn real_account_socket_replays_scoped_events_and_revokes_private_history_without_compute() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("explicit isolated PostgreSQL");
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("account_socket_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={search}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text NOT NULL,request_config jsonb NOT NULL,state text DEFAULT 'ready',desired_state text DEFAULT 'ready',runtime_state text DEFAULT 'ready',status_code text DEFAULT 'ready',revision bigint DEFAULT 1); CREATE TABLE cloud_agent_connections(owner_id text,connection_id text,state text,version bigint,account_label text,last_error_code text); CREATE TABLE cloud_auth_operations(owner_id text,actor_id text,operation_id text,state text,operation_ciphertext text,last_error_code text);").execute(&pool).await.unwrap();
    sqlx::raw_sql("CREATE TABLE cloud_environment_versions(owner_id text,project_id text,revision bigint); CREATE TABLE api_cloud_projects(project_id text,account_id text,state text,included boolean,configuration_digest text,last_error_code text); CREATE TABLE zuse_rust_github_operations(owner_id text,actor_id text,operation_id text,phase text); CREATE TABLE zuse_rust_github_users(actor_id text,state text,epoch bigint,version bigint); CREATE TABLE zuse_rust_github_installations(owner_id text,installation_id bigint,state text,revision bigint,allowed_repositories jsonb);").execute(&pool).await.unwrap();
    sqlx::raw_sql(crate::events::store::MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    for (id, creator) in [("visible", "actor"), ("hidden", "someone-else")] {
        sqlx::query("INSERT INTO api_cloud_workspaces(workspace_id,account_id,request_config) VALUES($1,'organization:team',$2)").bind(id).bind(json!({"sharingPolicy":policy(creator),"secret":"must-never-be-an-event"})).execute(&pool).await.unwrap();
    }
    let hub = EventHub::start(
        pool.clone(),
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([79; 32])).unwrap(),
        "https://staging.example.test".into(),
    )
    .await
    .unwrap();
    let identity = Arc::new(Member {
        active: AtomicBool::new(true),
    });
    let mut store = Store::from_pool(pool.clone());
    store.events = Some(hub.clone());
    let state = AppState {
        store,
        identity: identity.clone(),
        organizations_enabled: true,
        organization_ids: Arc::new(vec!["team".into()]),
        runtime: None,
        gateway: Arc::new(crate::gateway::Hub::default()),
    };
    let app = router(
        state,
        vec![HeaderValue::from_static("https://app.example.test")],
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    let device = uuid::Uuid::new_v4();
    let first_cursor = hub
        .cursors
        .encode(
            "actor",
            "organization:team",
            0,
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap();
    let ticket = new_ticket(&http, &origin, device, Some(&first_cursor)).await;
    // An unapproved browser cannot redeem the ticket, and this rejection happens
    // before consumption so the intended client can still redeem it.
    let mut wrong_origin = socket_request(&origin, &ticket);
    wrong_origin
        .headers_mut()
        .insert("Origin", "https://evil.example.test".parse().unwrap());
    assert!(
        tokio_tungstenite::connect_async(wrong_origin)
            .await
            .is_err()
    );
    let (mut socket, upgrade) = tokio_tungstenite::connect_async(socket_request(&origin, &ticket))
        .await
        .unwrap();
    assert_eq!(
        upgrade.headers().get("Sec-WebSocket-Protocol").unwrap(),
        PROTOCOL
    );
    assert!(
        tokio_tungstenite::connect_async(socket_request(&origin, &ticket))
            .await
            .is_err()
    );
    let first = frame(&mut socket).await;
    assert_eq!(first["type"], "events");
    assert_eq!(first["events"].as_array().unwrap().len(), 1);
    assert_eq!(first["events"][0]["resourceId"], "visible");
    assert!(!first.to_string().contains("someone-else"));
    assert!(!first.to_string().contains("must-never-be-an-event"));
    assert!(!first.to_string().contains("hidden"));
    let cursor = first["cursor"].as_str().unwrap().to_owned();
    socket
        .send(ClientMessage::Text(
            json!({"type":"ack","cursor":cursor}).to_string().into(),
        ))
        .await
        .unwrap();
    // The same physical org socket also sees its selected personal credential,
    // but no personal task or transcript metadata.
    let fresh_ticket = new_ticket(&http, &origin, uuid::Uuid::new_v4(), None).await;
    let (mut fresh, _) = tokio_tungstenite::connect_async(socket_request(&origin, &fresh_ticket))
        .await
        .unwrap();
    let ready = fresh.next().await.unwrap().unwrap().into_text().unwrap();
    let ready: Value = serde_json::from_str(&ready).unwrap();
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["revalidate"], true);
    fresh
        .send(ClientMessage::Text(
            json!({"type":"ack","cursor":ready["cursor"]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), fresh.next())
            .await
            .is_err(),
        "fresh device must not replay old status batches"
    );
    let _ = fresh.close(None).await;
    let mut personal_tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO cloud_agent_connections(owner_id,connection_id,state,version) VALUES('actor','personal-agent','connected',1)").execute(&mut *personal_tx).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_workspaces(workspace_id,account_id,request_config) VALUES('personal-private','actor','{}')").execute(&mut *personal_tx).await.unwrap();
    personal_tx.commit().await.unwrap();
    hub.wake();
    let credential = frame(&mut socket).await;
    assert_eq!(credential["events"].as_array().unwrap().len(), 1);
    assert_eq!(credential["events"][0]["resourceId"], "personal-agent");
    assert_eq!(credential["events"][0]["ownerId"], "actor");
    assert!(!credential.to_string().contains("personal-private"));
    let cursor = credential["cursor"].as_str().unwrap().to_owned();
    socket
        .send(ClientMessage::Text(
            json!({"type":"ack","cursor":cursor}).to_string().into(),
        ))
        .await
        .unwrap();
    let _ = socket.close(None).await;
    sqlx::query(
        "UPDATE api_cloud_workspaces SET state='sleeping',revision=2 WHERE workspace_id='visible'",
    )
    .execute(&pool)
    .await
    .unwrap();
    hub.wake();
    let ticket = new_ticket(&http, &origin, device, Some(&cursor)).await;
    let (mut resumed, _) = tokio_tungstenite::connect_async(socket_request(&origin, &ticket))
        .await
        .unwrap();
    let next = frame(&mut resumed).await;
    assert_eq!(next["events"].as_array().unwrap().len(), 1);
    assert_eq!(next["events"][0]["payload"]["revision"], 2);
    let cursor = next["cursor"].as_str().unwrap().to_owned();
    assert_ne!(cursor, first["cursor"]);
    resumed
        .send(ClientMessage::Text(
            json!({"type":"ack","cursor":cursor}).to_string().into(),
        ))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE api_cloud_workspaces SET request_config=$1,revision=3 WHERE workspace_id='visible'",
    )
    .bind(json!({"sharingPolicy":policy("someone-else")}))
    .execute(&pool)
    .await
    .unwrap();
    hub.wake();
    let revoked = frame(&mut resumed).await;
    assert_eq!(revoked["events"], json!([]));
    assert_eq!(revoked["revokedWorkspaceIds"], json!(["visible"]));
    resumed
        .send(ClientMessage::Text(
            json!({"type":"ack","cursor":revoked["cursor"]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    identity.active.store(false, Ordering::SeqCst);
    sqlx::query("UPDATE api_cloud_workspaces SET revision=4 WHERE workspace_id='visible'")
        .execute(&pool)
        .await
        .unwrap();
    hub.wake();
    let denied = frame(&mut resumed).await;
    assert_eq!(denied["type"], "error");
    assert_eq!(denied["cacheRevoked"], true);
    let _ = resumed.close(None).await;
    server.abort();
    let _ = server.await;
    drop(hub);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
