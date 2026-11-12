//! Local interactive plugin API. Deliberately separate from the production binary.
use anyhow::{Context, ensure};
use async_trait::async_trait;
use axum::{
    Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::Arc;
use zuse_cloud::{
    auth::{Identity, Membership, Principal},
    crypto::MasterKey,
    error::ApiError,
    plugins::{PluginConfig, PluginControl, egress::Policy},
    routes::{AppState, router},
    store::Store,
};

struct LocalIdentity;
#[async_trait]
impl Identity for LocalIdentity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        // Only the exact fixture JWT produced for this local subject is accepted.
        // This is a separate loopback example, never production authentication.
        let fixture = token
            .strip_prefix("fixture.")
            .and_then(|s| s.strip_suffix(".signature"))
            .and_then(|payload| {
                use base64::Engine as _;
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(payload)
                    .ok()
            })
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|claims| {
                claims["sub"] == "local-user"
                    && claims["exp"]
                        .as_i64()
                        .is_some_and(|exp| exp > chrono::Utc::now().timestamp())
            });
        if token != "local-dev" && !fixture {
            return Err(ApiError::unauthorized("invalid_local_token"));
        }
        Ok(Principal {
            account_id: "local-user".into(),
        })
    }
    async fn membership(&self, _: &str, _: &str) -> Result<Membership, ApiError> {
        Err(ApiError::forbidden("local_personal_only"))
    }
}

async fn echo(Json(input): Json<Value>) -> Response {
    let Some(id) = input.get("id") else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match input["method"].as_str().unwrap_or("") {
        "initialize" => json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"local-echo","version":"1"}}),
        "ping" => json!({}),
        "tools/list" => json!({"tools":[{"name":"echo","description":"Return your message","inputSchema":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false},"annotations":{"readOnlyHint":true}}]}),
        "tools/call" if input["params"]["name"] == "echo" => json!({"content":[{"type":"text","text":input["params"]["arguments"]["message"].as_str().unwrap_or("")}]}),
        _ => return Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown method or tool"}})).into_response(),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .context("Set a dedicated localhost ZUSE_CLOUD_TEST_DATABASE_URL")?;
    let parsed = reqwest::Url::parse(&url)?;
    ensure!(
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")),
        "Local database required"
    );
    let key = MasterKey::parse(
        &std::env::var("ZUSE_CLOUD_PLUGIN_ENCRYPTION_KEY")
            .context("Set a stable 32-byte base64 plugin key")?,
    )
    .map_err(|_| anyhow::anyhow!("Invalid plugin key"))?;
    let pool = sqlx::PgPool::connect(&url).await?;
    // A dedicated schema keeps interactive state across restarts without touching other tables.
    sqlx::raw_sql("CREATE SCHEMA IF NOT EXISTS plugin_local")
        .execute(&pool)
        .await?;
    pool.close().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET search_path = plugin_local")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await?;
    let present: bool =
        sqlx::query_scalar("SELECT to_regclass('api_cloud_workspaces') IS NOT NULL")
            .fetch_one(&pool)
            .await?;
    if !present {
        sqlx::raw_sql(include_str!("../tests/fixtures/legacy-catalog.sql"))
            .execute(&pool)
            .await?;
    }
    PluginControl::migrate(&pool, &key)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.code))?;
    let host = PluginControl::from_pool(
        pool.clone(),
        PluginConfig {
            key,
            public_origin: "http://127.0.0.1:8088".parse()?,
            app_origin: "http://127.0.0.1:8088".parse()?,
            policy: Policy::Loopback,
        },
    )?;
    let state = AppState {
        store: Store::from_pool(pool).with_plugin_control(host),
        identity: Arc::new(LocalIdentity),
        organizations_enabled: false,
        organization_ids: Arc::new(vec![]),
        runtime: None,
        gateway: Arc::new(zuse_cloud::gateway::Hub::default()),
    };
    let app = router(state, vec![]).merge(Router::new().route("/dev/mcp", post(echo)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8088").await?;
    println!("Local plugin API: http://127.0.0.1:8088 (Bearer local-dev)");
    println!("Tenant: personal:local-user; echo MCP: http://127.0.0.1:8088/dev/mcp");
    println!(
        "Development fixture identity only; OAuth/WorkOS acceptance uses the production binary."
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
