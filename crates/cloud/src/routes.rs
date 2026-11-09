mod account_events;
#[cfg(test)]
mod account_events_test;
#[cfg(test)]
mod github_browser_policy_test;
mod github_organizations;
mod recreations;
mod task_commands;
use crate::{
    access::{self, Access, AccessKind},
    archive,
    auth::Identity,
    error::ApiError,
    gateway::Hub,
    runtime::{BootstrapAck, BootstrapRequest, RenewRequest, RuntimeControl},
    store::Store,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State, WebSocketUpgrade, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub identity: Arc<dyn Identity>,
    pub organizations_enabled: bool,
    pub organization_ids: Arc<Vec<String>>,
    pub runtime: Option<Arc<RuntimeControl>>,
    pub gateway: Arc<Hub>,
}

impl AppState {
    async fn access(&self, headers: &HeaderMap, kind: AccessKind) -> Result<Access, ApiError> {
        let access = self.setup_access(headers, kind).await?;
        // Native configured servers apply the same departure/enrollment gate to
        // fresh history and settings as to execution. Keep read-only bridge
        // fixtures/deployments compatible until that authority is configured.
        if access.membership.is_some() && !matches!(kind, AccessKind::Billing) {
            if let Some(runtime) = self
                .runtime
                .as_ref()
                .filter(|r| r.scope_authority_configured())
            {
                runtime.scope_admission(&access).await?;
            }
        }
        Ok(access)
    }
    async fn setup_access(
        &self,
        headers: &HeaderMap,
        kind: AccessKind,
    ) -> Result<Access, ApiError> {
        access::authorize(
            self.identity.as_ref(),
            headers,
            kind,
            self.organizations_enabled,
            &self.organization_ids,
        )
        .await
    }
}

/// Dedicated relay listener: no account, billing, credential or lifecycle HTTP API.
/// Ticket issuance remains the responsibility of the compatible control-plane API.
pub fn gateway_router(state: AppState, origins: Vec<HeaderValue>) -> Router {
    let origins = Arc::new(origins);
    let gateway_origins = origins.clone();
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route(
            "/v1/cloud/workspaces/{id}/gateway",
            get(move |state, path, headers: HeaderMap, ws| {
                let origins = gateway_origins.clone();
                async move { gateway_upgrade(state, path, headers, ws, origins).await }
            }),
        )
        .layer(middleware::map_response(private_response))
        .layer(
            CorsLayer::new()
                .allow_origin(origins.as_ref().clone())
                .allow_methods([Method::GET])
                .allow_headers([header::AUTHORIZATION]),
        )
        .with_state(state)
}

#[cfg(test)]
mod gateway_listener_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    struct UnusedIdentity;
    #[async_trait::async_trait]
    impl Identity for UnusedIdentity {
        async fn verify(&self, _: &str) -> Result<crate::auth::Principal, ApiError> {
            panic!("HTTP account authentication must not be reachable on the relay listener")
        }
        async fn membership(&self, _: &str, _: &str) -> Result<crate::auth::Membership, ApiError> {
            panic!("HTTP organization lookup must not be reachable on the relay listener")
        }
    }

    #[tokio::test]
    async fn gateway_listener_excludes_control_plane_routes() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let app = gateway_router(
            AppState {
                store: Store::from_pool(pool),
                identity: Arc::new(UnusedIdentity),
                organizations_enabled: false,
                organization_ids: Arc::new(vec![]),
                runtime: None,
                gateway: Arc::new(Hub::default()),
            },
            vec![],
        );
        for (method, path) in [
            ("GET", "/v1/cloud/context"),
            ("GET", "/v1/cloud/workspaces"),
            ("POST", "/v1/cloud/workspaces"),
            ("POST", "/v1/cloud/workspaces/test/gateway/ticket"),
            ("POST", "/v1/cloud/workspaces/test/runtime/bootstrap"),
            ("POST", "/v1/cloud/workspaces/test/resume"),
            ("GET", "/v1/cloud/billing/summary"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/cloud/workspaces/test/gateway")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // The WebSocket route is registered but rejects a request without upgrade headers.
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

pub fn router(state: AppState, origins: Vec<HeaderValue>) -> Router {
    let github_webhook_gate = Arc::new(tokio::sync::Semaphore::new(1));
    let origins = Arc::new(origins);
    let gateway_origins = origins.clone();
    let event_origins = origins.clone();
    let account_events = state.store.events.clone();
    let cors = CorsLayer::new()
        .allow_origin(origins.as_ref().clone())
        .allow_methods(
            if state.runtime.is_some()
                || state.store.devices.is_some()
                || state.store.billing_writes_enabled()
                || state.store.project_writes_enabled()
                || state.store.image_writes_enabled()
                || state.store.connections.is_some()
                || state.store.computers.is_some()
            {
                vec![Method::GET, Method::POST, Method::PUT, Method::DELETE]
            } else {
                vec![Method::GET]
            },
        )
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("dpop"),
            header::HeaderName::from_static("x-zuse-workspace"),
            header::HeaderName::from_static("x-zuse-device-id"),
        ]);
    let mut app = Router::new()
        .route(
            "/v1/cloud/events/ticket",
            post(account_events::ticket).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/v1/cloud/events",
            get(move |state, headers, ws| {
                let origins = event_origins.clone();
                async move { account_events::upgrade(state, headers, ws, origins).await }
            }),
        )
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/environments", get(environments))
        .route(
            "/v1/client/environment-links",
            post(native_link).layer(DefaultBodyLimit::max(32768)),
        )
        .route(
            "/v1/client/environment-link-challenges",
            post(native_link_challenge),
        )
        .route(
            "/v1/environments/{id}/heartbeat",
            post(native_heartbeat).layer(DefaultBodyLimit::max(32768)),
        )
        .route(
            "/v1/environments/{id}/connect",
            post(native_connect).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/v1/mobile/devices",
            post(register_native_device).layer(DefaultBodyLimit::max(8192)),
        )
        .route(
            "/v1/client/dpop-token",
            post(device_access_token).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/v1/cloud/context", get(context))
        .route("/v1/cloud/capabilities", get(computer_capabilities))
        .route(
            "/v1/cloud/projects/{project}/secrets",
            get(list_workload_secrets).post(save_workload_secret),
        )
        .route(
            "/v1/cloud/projects/{project}/secrets/{secret}",
            axum::routing::delete(revoke_workload_secret),
        )
        .route(
            "/v1/cloud/workspaces/{id}/runtime/workload-secrets/grant",
            post(workload_secret_grant),
        )
        .route("/v1/cloud/agents", get(agent_connections))
        .route(
            "/v1/cloud/agents/configure",
            post(configure_agent_connection).layer(DefaultBodyLimit::max(40000)),
        )
        .route("/v1/cloud/agents/login", post(start_agent_login))
        .route(
            "/v1/cloud/agents/operations/{id}",
            get(agent_login_operation).delete(cancel_agent_login),
        )
        .route(
            "/v1/cloud/agents/{id}",
            axum::routing::delete(disconnect_agent),
        )
        .route(
            "/v1/cloud/projects/{id}/environment/preparation",
            get(project_environment_preparation),
        )
        .route(
            "/v1/cloud/projects/{id}/environment/preparations/{request}/authorize",
            post(authorize_project_preparation),
        )
        .route(
            "/v1/cloud/projects/{id}/environment/preparations/{request}/source-credential",
            post(project_preparation_source_credential),
        )
        .route(
            "/v1/cloud/machine",
            get(machine)
                .put(save_machine)
                .layer(DefaultBodyLimit::max(1536 * 1024)),
        )
        .route(
            "/v1/cloud/projects/{id}/environment",
            get(project_environment)
                .put(save_project_environment)
                .layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/v1/cloud/launch-assets/chunk",
            post(launch_asset_chunk).layer(DefaultBodyLimit::max(800_000)),
        )
        .route(
            "/v1/cloud/launch-assets/commit",
            post(launch_asset_commit).layer(DefaultBodyLimit::max(8192)),
        )
        .route(
            "/v1/cloud/workspaces/{id}/runtime/attachments/{asset}",
            get(runtime_asset),
        )
        .route(
            "/v1/cloud/workspaces/{id}/allocation-scope",
            post(allocation_scope),
        )
        .route(
            "/v1/cloud/workspaces/{id}/allocation-source",
            post(private_project_allocation_source),
        )
        .route(
            "/v1/cloud/workspaces/{id}/worker-effect-scope",
            post(worker_effect_scope),
        )
        .merge(github_organizations::routes())
        .route("/v1/cloud/github", get(github_catalog))
        .route("/v1/cloud/github/install", post(github_install))
        .route("/v1/cloud/github/connect", get(github_connect))
        .route("/v1/cloud/github/callback", get(github_callback))
        .route("/v1/cloud/github/setup", get(github_setup))
        .route("/v1/cloud/github/browser", get(github_browser_page))
        .route(
            "/v1/cloud/github/browser/identity",
            post(github_browser_identity).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/v1/cloud/github/browser/select",
            post(github_browser_select).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/v1/cloud/github/status", get(github_status))
        .route(
            "/v1/cloud/github/select",
            post(github_select).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/v1/cloud/github/cancel",
            post(github_cancel).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/v1/cloud/github/disconnect", post(github_disconnect))
        .route(
            "/v1/cloud/github/remove",
            post(github_remove).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/v1/cloud/github/pull-requests",
            post(github_pull_request).layer(DefaultBodyLimit::max(70 * 1024)),
        )
        .route(
            "/v1/cloud/workspaces/{id}/runtime/github/grant",
            post(runtime_github_grant).layer(DefaultBodyLimit::max(24 * 1024)),
        )
        .route(
            "/v1/cloud/github/webhook",
            post(github_webhook)
                .layer(DefaultBodyLimit::max(
                    crate::github::native::GITHUB_WEBHOOK_BODY_LIMIT,
                ))
                // This outer layer acquires the permit before Bytes extraction.
                .layer(middleware::from_fn_with_state(
                    github_webhook_gate,
                    github_webhook_admission,
                )),
        )
        .route("/v1/cloud/image", get(image_status))
        .route(
            "/v1/cloud/image/delete",
            post(image_delete).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/v1/cloud/image/build",
            post(image_build).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/v1/cloud/projects",
            get(project_list)
                .post(project_connect)
                .layer(DefaultBodyLimit::max(160_000)),
        )
        .route(
            "/v1/cloud/projects/{id}",
            axum::routing::delete(project_remove),
        )
        .route("/v1/billing/entitlements", get(entitlements))
        .route("/v1/billing/portal", post(billing_portal))
        .route("/v1/billing/prepaid", get(prepaid_balance))
        .route(
            "/v1/billing/prepaid/checkout",
            post(prepaid_checkout).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/v1/billing/prepaid/complete", get(prepaid_complete))
        .route(
            "/v1/billing/checkout",
            post(billing_checkout).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/v1/billing/checkout/complete", get(checkout_complete))
        .route(
            "/v1/billing/webhook",
            post(billing_webhook).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route(
            "/v1/billing/webhook/{provider}",
            post(billing_webhook_provider).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/v1/cloud/billing/summary", get(billing_summary))
        .route("/v1/cloud/billing/usage", get(billing_usage))
        .route(
            "/v1/cloud/billing/cap",
            post(billing_cap).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/v1/cloud/workspaces", get(workspaces))
        .route("/v1/cloud/workspaces/{id}", get(workspace))
        .route(
            "/v1/cloud/workspaces/{id}/task-commands",
            post(task_commands::submit).layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .route(
            "/v1/cloud/workspaces/{id}/task-commands/queue",
            get(task_commands::queue),
        )
        .route(
            "/v1/cloud/workspaces/{id}/task-commands/{command}",
            get(task_commands::status),
        )
        .route(
            "/v1/cloud/workspaces/{id}/sessions",
            get(task_commands::sessions),
        )
        .route(
            "/v1/cloud/workspaces/{id}/assets/chunks",
            post(task_commands::asset_chunk).layer(DefaultBodyLimit::max(768 * 1024)),
        )
        .route(
            "/v1/cloud/workspaces/{id}/assets/commit",
            post(task_commands::asset_commit).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/v1/cloud/workspaces/{id}/assets/{asset}/chunks",
            get(task_commands::asset_read),
        )
        .route(
            "/v1/cloud/workspaces/{id}/sessions/{session}/task-history",
            get(task_history),
        )
        .route(
            "/v1/cloud/workspaces/{id}/preparation-logs",
            get(preparation_logs),
        )
        .route(
            "/v1/cloud/workspaces/{id}/tool-output",
            get(tool_output_read),
        )
        .route(
            "/v1/cloud/workspaces/{id}/archive-status",
            get(archive_status),
        );
    if state.store.transcripts_enabled() {
        app = app
            .route(
                "/v1/cloud/workspaces/{id}/sessions/{session}/transcript-checkpoint",
                get(transcript_checkpoint),
            )
            .route(
                "/v1/cloud/workspaces/{id}/sessions/{session}/transcript-message-page",
                get(transcript_page),
            );
    }
    if state
        .runtime
        .as_ref()
        .is_some_and(|runtime| runtime.creation_enabled())
    {
        app = app.route(
            "/v1/cloud/workspaces",
            post(workspace_create).layer(DefaultBodyLimit::max(256 * 1024)),
        );
    }
    if state.runtime.is_some() {
        app = app
            .route("/v1/cloud/workspaces/{id}/delete", post(workspace_delete))
            .route(
                "/v1/cloud/workspaces/{id}/computer-recreations",
                post(recreations::start).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/computer-recreations/{operation}",
                get(recreations::status),
            )
            .route(
                "/v1/cloud/workspaces/{id}/computer-recreations/{operation}/retry",
                post(recreations::retry).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime-repairs",
                post(runtime_repair_start).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime-repairs/{operation}",
                get(runtime_repair_status),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime-repairs/{operation}/resolve",
                post(runtime_repair_resolve).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/allocations/{operation}/retry",
                post(allocation_retry).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime-repairs/{operation}/retry",
                post(runtime_repair_retry).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/archive/{operation}/{object}",
                get(archive_object_get)
                    .put(archive_object_put)
                    .layer(DefaultBodyLimit::max(crate::runtime::recovery::WIRE_LIMIT)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/transcript-checkpoint",
                post(transcript_upload)
                    .layer(DefaultBodyLimit::max(crate::transcript::HEAD_LIMIT + 4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/transcript-message-page",
                post(transcript_page_upload)
                    .layer(DefaultBodyLimit::max(crate::transcript::PAGE_LIMIT + 4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/pause",
                post(workspace_pause).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/archive",
                post(workspace_archive).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/unarchive",
                post(workspace_unarchive).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/resume",
                post(workspace_resume).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/lifecycle",
                post(runtime_lifecycle).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/providers/{provider}/grant",
                post(provider_grant).layer(DefaultBodyLimit::max(24 * 1024)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/commands",
                post(command_enqueue).layer(DefaultBodyLimit::max(
                    zuse_protocol::cloud_commands::MAX_ENVELOPE_BYTES,
                )),
            )
            .route(
                "/v1/cloud/workspaces/{id}/commands/watch",
                get(command_watch),
            )
            .route(
                "/v1/cloud/workspaces/{id}/commands/{command}",
                get(command_status).delete(command_cancel),
            )
            .route("/v1/cloud/workspaces/{id}/data-key", get(command_data_key))
            .route(
                "/v1/cloud/workspaces/{id}/runtime/commands/progress",
                post(task_commands::progress).layer(DefaultBodyLimit::max(16 * 1024)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/commands/lease",
                post(command_lease).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/commands/ack",
                post(command_ack).layer(DefaultBodyLimit::max(
                    zuse_protocol::cloud_commands::MAX_ENVELOPE_BYTES,
                )),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/storage/enroll",
                post(runtime_storage).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/ready",
                post(runtime_ready).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/preparation/ingress",
                post(runtime_preparation_ingress).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/preparation",
                post(runtime_preparation).layer(DefaultBodyLimit::max(128 * 1024)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/tool-output",
                post(tool_output_upload).layer(DefaultBodyLimit::max(256 * 1024)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/gateway/ticket",
                post(gateway_ticket).layer(DefaultBodyLimit::max(1024)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/gateway",
                get(move |state, path, headers: HeaderMap, ws| {
                    let origins = gateway_origins.clone();
                    async move { gateway_upgrade(state, path, headers, ws, origins).await }
                }),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/bootstrap",
                post(runtime_bootstrap).layer(DefaultBodyLimit::max(64 * 1024)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/credentials/renew",
                post(runtime_renew).layer(DefaultBodyLimit::max(24 * 1024)),
            )
            .route(
                "/v1/cloud/workspaces/{id}/runtime/bootstrap/ack",
                post(runtime_ack).layer(DefaultBodyLimit::max(4096)),
            );
    }
    app.layer(middleware::from_fn(
        move |request: axum::extract::Request, next: middleware::Next| {
            let events = account_events.clone();
            async move {
                let changed = matches!(
                    *request.method(),
                    Method::POST | Method::PUT | Method::DELETE | Method::PATCH
                );
                let response = next.run(request).await;
                if changed && response.status().is_success() {
                    if let Some(events) = events {
                        events.wake();
                    }
                }
                response
            }
        },
    ))
    .layer(middleware::map_response(private_response))
    .layer(cors)
    .with_state(state)
}

async fn private_response(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(
        json!({"status":"ok","service":"zuse-cloud-api","version":env!("CARGO_PKG_VERSION"),"mode":mode(&state)}),
    )
}
async fn ready(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    state.store.ready().await?;
    if let Some(runtime) = &state.runtime {
        runtime.ready().await?;
    }
    Ok(Json(json!({"status":"ready","mode":mode(&state)})))
}
fn mode(state: &AppState) -> &'static str {
    if state.runtime.is_some() {
        "runtime-control"
    } else {
        "read-only"
    }
}

async fn gateway_ticket(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    // Use the catalog authorization first so other owners remain indistinguishable.
    state.store.workspace(&access, &id).await?;
    let body: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body)
            .map_err(|_| ApiError::bad_request("invalid_workspace_gateway_protocol"))?
    };
    if !body.is_object() {
        return Err(ApiError::bad_request("invalid_workspace_gateway_protocol"));
    }
    let protocol = match body.get("protocol") {
        None => zuse_protocol::cloud_gateway::PROTOCOL,
        Some(value) => value
            .as_str()
            .ok_or_else(|| ApiError::bad_request("invalid_workspace_gateway_protocol"))?,
    };
    let device = headers
        .get("x-zuse-device-id")
        .map(|v| {
            v.to_str()
                .map_err(|_| ApiError::bad_request("invalid_device_id"))
        })
        .transpose()?
        .unwrap_or(&access.actor.account_id);
    let control = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        control
            .gateway_ticket(
                &id,
                &access,
                protocol,
                body.get("rpcProtocol")
                    .map(|p| {
                        p.as_str()
                            .ok_or_else(|| ApiError::bad_request("invalid_workspace_rpc_protocol"))
                    })
                    .transpose()?,
                device,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn gateway_upgrade(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    origins: Arc<Vec<HeaderValue>>,
) -> Result<Response, ApiError> {
    // Cookies and forwarded gateway headers never grant authority. Browser origins
    // must be explicitly configured; native guests carry no Origin header.
    if headers
        .get(header::ORIGIN)
        .is_some_and(|origin| !origins.contains(origin))
    {
        return Err(ApiError::forbidden("workspace_gateway_origin_rejected"));
    }
    let values: Vec<_> = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .collect();
    if values.len() != 2
        || ![
            zuse_protocol::cloud_gateway::PROTOCOL,
            zuse_protocol::cloud_gateway::PENDING_PROTOCOL,
            zuse_protocol::cloud_gateway::LEGACY_PROTOCOL,
            zuse_protocol::cloud_gateway::RUST_PROTOCOL,
        ]
        .contains(&values[0])
    {
        return Err(ApiError::unauthorized("workspace_gateway_rejected"));
    }
    let control = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?
        .clone();
    let storage = headers
        .get(zuse_protocol::cloud::STARTUP_BIND_HEADER)
        .map(|value| {
            if value.as_bytes().len() > 4096 {
                return Err(ApiError::bad_request("invalid_runtime_storage_binding"));
            }
            serde_json::from_slice::<zuse_protocol::cloud::CloudStorageEnrollment>(value.as_bytes())
                .map_err(|_| ApiError::bad_request("invalid_runtime_storage_binding"))
        })
        .transpose()?;
    let grant = control
        .gateway_authorize_with_storage(
            &id,
            values[0],
            values[1],
            chrono::Utc::now().timestamp_millis(),
            storage,
        )
        .await?;
    let permit = state.gateway.permit()?;
    let room = state.gateway.room(control, &grant).await?;
    let max_frame = if grant.protocol == zuse_protocol::cloud_gateway::LEGACY_PROTOCOL {
        zuse_protocol::cloud_gateway::MAX_FRAME * 4 / 3 + 1024
    } else {
        zuse_protocol::cloud_gateway::MAX_FRAME
    };
    Ok(ws
        .protocols([grant.protocol.clone()])
        .max_message_size(max_frame)
        .max_frame_size(max_frame)
        .on_upgrade(move |socket| room.serve(grant, socket, permit)))
}
async fn runtime_storage(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud::CloudStorageEnrollment>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("invalid_runtime_storage_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .enroll_storage(
                &id,
                runtime_bearer(&headers)?,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn runtime_ready(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud::CloudRuntimeReady>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_runtime_ready_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .ready_launch(
                &id,
                runtime_bearer(&headers)?,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn runtime_preparation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_computers::RuntimePreparationUpdate>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("invalid_runtime_preparation_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .preparation_update(
                &id,
                runtime_bearer(&headers)?,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn runtime_bootstrap(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<BootstrapRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let token = runtime_bearer(&headers)?;
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("invalid_runtime_bootstrap_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .bootstrap(&id, token, request, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn runtime_renew(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<RenewRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let token = runtime_bearer(&headers)?;
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("invalid_runtime_renewal_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .renew(&id, token, request, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn runtime_ack(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<BootstrapAck>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let token = runtime_bearer(&headers)?;
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("invalid_runtime_bootstrap_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    runtime
        .acknowledge(&id, token, request, chrono::Utc::now().timestamp_millis())
        .await?;
    Ok(Json(json!({"acknowledged":true})))
}
fn runtime_bearer(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() && token.len() <= 512
        })
        .map(|(_, token)| token)
        .ok_or_else(|| ApiError::unauthorized("workspace_runtime_rejected"))
}
async fn context(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let mut projects = state.store.launch_projects(&access.owner_id).await?;
    if let Some(github) = state
        .store
        .native_github()
        .filter(|g| g.enabled(&access.owner_id))
    {
        let catalog = github.catalog(&access).await?;
        crate::github::native::mark_project_access(&mut projects, &catalog);
    }
    let mut connection_metadata = Vec::new();
    let connections = if state.store.connections.is_some() {
        let mut connections = state.store.agent_connections(&access.owner_id).await?;
        for connection in &connections {
            let mut value = serde_json::to_value(connection)
                .map_err(|_| ApiError::unavailable("agent_connection_metadata_invalid"))?;
            value["connectionScope"] = json!(if access.membership.is_some() {
                "organization"
            } else {
                "personal"
            });
            connection_metadata.push(value);
        }
        if access.membership.is_some() && access.owner_id != access.actor.account_id {
            let personal = state
                .store
                .agent_connections(&access.actor.account_id)
                .await?;
            for connection in &personal {
                let mut value = serde_json::to_value(connection)
                    .map_err(|_| ApiError::unavailable("agent_connection_metadata_invalid"))?;
                value["connectionScope"] = json!("personal");
                connection_metadata.push(value);
            }
            connections.extend(personal);
        }
        Some(connections)
    } else {
        None
    };
    let catalog = if let Some(connections) = &connections {
        state
            .store
            .managed_launch_catalog_for(&access.owner_id, connections)
            .await?
    } else {
        crate::model_catalog::launch_catalog()
    };
    let provider_catalogs = if let Some(connections) = &connections {
        serde_json::to_value(
            state
                .store
                .provider_launch_catalogs_for(&access.owner_id, connections)
                .await?,
        )
        .map_err(|_| ApiError::unavailable("agent_catalog_invalid"))?
    } else {
        Value::Null
    };
    let capabilities = state
        .store
        .computer_capabilities_for_actor(
            &access.owner_id,
            &access.actor.account_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    let command_admission_version = if let Some(runtime) = &state.runtime {
        if runtime.creation_enabled_for(&access.owner_id)
            && runtime.task_command_creation_available().await?
        {
            Some(zuse_protocol::cloud_task_commands::VERSION)
        } else {
            None
        }
    } else {
        None
    };
    Ok(Json(
        json!({"actor":access.actor,"scope":access.scope,"ownerId":access.owner_id,"accountEvents":state.store.events.is_some(),"projects":projects,"agentCatalog":catalog,"providerAgentCatalogs":provider_catalogs,"computerCapabilities":capabilities,"commandAdmissionVersion":command_admission_version,"agentConnections":if connections.is_some(){json!(connection_metadata)}else{Value::Null}}),
    ))
}

async fn agent_connections(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        json!({"connections":state.store.agent_connections(&access.owner_id).await?,"supportsDeviceReconnect":true,"supportsCredentialReconfiguration":true}),
    ))
}
async fn configure_agent_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_connections::ConfigureConnection>, JsonRejection>,
) -> Result<Json<zuse_protocol::cloud_connections::AgentConnection>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_agent_connection"))?;
    let control = state
        .store
        .connections
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_agent_connection_writes_disabled"))?;
    Ok(Json(
        control
            .configure(
                &access.owner_id,
                &access.actor.account_id,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn start_agent_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_connections::StartConnectionLogin>, JsonRejection>,
) -> Result<Json<zuse_protocol::cloud_connections::LoginOperation>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_device_login"))?;
    let control = state
        .store
        .connections
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_agent_connection_writes_disabled"))?;
    Ok(Json(
        control
            .start_login(
                &access.owner_id,
                &access.actor.account_id,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn agent_login_operation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<zuse_protocol::cloud_connections::LoginOperation>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let control = state
        .store
        .connections
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_agent_connection_writes_disabled"))?;
    Ok(Json(
        control
            .operation(&access.owner_id, &id, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn cancel_agent_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<zuse_protocol::cloud_connections::LoginOperation>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let control = state
        .store
        .connections
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_agent_connection_writes_disabled"))?;
    Ok(Json(
        control
            .cancel(&access.owner_id, &id, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn disconnect_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let control = state
        .store
        .connections
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_agent_connection_writes_disabled"))?;
    control
        .disconnect(&access.owner_id, &id, chrono::Utc::now().timestamp_millis())
        .await?;
    Ok(Json(json!({"ok":true})))
}

async fn computer_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<zuse_protocol::cloud_computers::CloudCapabilities>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let mut capabilities = state
        .store
        .computer_capabilities_for_actor(
            &access.owner_id,
            &access.actor.account_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    if !state
        .runtime
        .as_ref()
        .is_some_and(|runtime| runtime.creation_enabled_for(&access.owner_id))
    {
        capabilities.can_allocate = false;
        capabilities
            .allocation_blockers
            .push("cloud_workspace_creation_disabled".into());
    }
    Ok(Json(capabilities))
}
async fn machine(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<zuse_protocol::cloud_computers::MachineVersion>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(state.store.machine(&access.owner_id).await?))
}
async fn save_machine(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_computers::SaveMachine>, JsonRejection>,
) -> Result<Json<zuse_protocol::cloud_computers::MachineVersion>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_machine_recipe"))?;
    Ok(Json(
        state
            .store
            .save_machine_scoped(&access, request, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn project_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<zuse_protocol::cloud_computers::EnvironmentVersion>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        state
            .store
            .project_environment(&access.owner_id, &id)
            .await?,
    ))
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreparationAuthorizationQuery {
    github_user_id: Option<i64>,
}
async fn authorize_project_preparation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<PreparationAuthorizationQuery>,
) -> Result<StatusCode, ApiError> {
    let proof = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("project_preparation_scope_denied"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("project_preparation_authority_unavailable"))?;
    let now = chrono::Utc::now().timestamp_millis();
    if let Some(source_proof) = headers.get("x-zuse-project-source-proof") {
        let source_proof = source_proof
            .to_str()
            .map_err(|_| ApiError::forbidden("project_cache_source_grant_invalid"))?;
        let user = query
            .github_user_id
            .ok_or_else(|| ApiError::forbidden("project_cache_source_grant_invalid"))?;
        runtime
            .revalidate_project_source(&id, &request, proof, now, user, source_proof)
            .await?;
    } else {
        runtime
            .authorize_project_preparation_user(&id, &request, proof, now, query.github_user_id)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn project_preparation_source_credential(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let proof = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("project_preparation_scope_denied"))?;
    let grant = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("project_preparation_authority_unavailable"))?
        .project_preparation_source_credential(
            &id,
            &request,
            proof,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(grant),
    ))
}
async fn private_project_allocation_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let proof = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("project_cache_source_authorization_revoked"))?;
    let source = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("project_cache_source_authority_unavailable"))?
        .private_project_allocation_source(&id, proof, chrono::Utc::now().timestamp_millis())
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(source),
    ))
}
async fn project_environment_preparation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        state
            .store
            .project_preparation(&access.owner_id, &id)
            .await?,
    ))
}
async fn save_project_environment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<zuse_protocol::cloud_computers::SaveEnvironment>, JsonRejection>,
) -> Result<Json<zuse_protocol::cloud_computers::EnvironmentVersion>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_environment_recipe"))?;
    Ok(Json(
        state
            .store
            .save_project_environment_scoped(
                &access,
                &id,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn entitlements(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    Ok(Json(
        json!({"entitlements":state.store.entitlements(&access.owner_id).await?}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceQuery {
    project_id: Option<String>,
}
async fn native_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    if !matches!(access.scope, access::Scope::Personal) {
        return Err(ApiError::forbidden("environment_scope_not_supported"));
    }
    let authority = state
        .store
        .devices
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("device_authority_disabled"))?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_environment_link"))?;
    Ok(Json(authority.link(&access.actor.account_id, &body).await?))
}

async fn native_link_challenge(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    if !matches!(access.scope, access::Scope::Personal) {
        return Err(ApiError::forbidden("environment_scope_not_supported"));
    }
    let authority = state
        .store
        .devices
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("device_authority_disabled"))?;
    Ok(Json(
        authority.link_challenge(&access.actor.account_id).await?,
    ))
}

async fn native_heartbeat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let token = runtime_bearer(&headers)?;
    if !token.starts_with("zenv_") {
        return Err(ApiError::unauthorized("invalid_environment_credential"));
    }
    let authority = state
        .store
        .devices
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("device_authority_disabled"))?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_heartbeat"))?;
    Ok(Json(authority.heartbeat(&id, token, &body).await?))
}

async fn native_connect(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (_, token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("DPoP") && !token.is_empty())
        .ok_or_else(|| ApiError::unauthorized("missing_device_token"))?;
    let proof = headers
        .get("dpop")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing_dpop"))?;
    let authority = state
        .store
        .devices
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("device_authority_disabled"))?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_connection_request"))?;
    if body["requireManaged"] != true || body["wireProtocolVersion"] != 1 {
        return Err(ApiError::bad_request("managed_protocol_required"));
    }
    Ok(Json(authority.connect(&id, token, proof).await?))
}

async fn register_native_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("DPoP") && !token.is_empty())
        .ok_or_else(|| ApiError::unauthorized("missing_device_token"))?;
    let proof = headers
        .get("dpop")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing_dpop"))?;
    let authority = state
        .store
        .devices
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("device_authority_disabled"))?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_device"))?;
    Ok(Json(
        authority.register(authorization.1, proof, &body).await?,
    ))
}

async fn device_access_token(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    if !matches!(access.scope, access::Scope::Personal) {
        return Err(ApiError::forbidden("device_scope_not_supported"));
    }
    let proof = headers
        .get("dpop")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing_dpop"))?;
    let authority = state
        .store
        .devices
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("device_authority_disabled"))?;
    Ok(Json(authority.mint(&access.actor.account_id, proof).await?))
}

async fn environments(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    // Organization sharing needs its own audience policy; never infer visibility
    // from organization membership alone.
    if !matches!(access.scope, access::Scope::Personal) {
        return Err(ApiError::forbidden("environment_scope_not_supported"));
    }
    Ok(Json(
        json!({"environments": state.store.personal_environments(&access.actor.account_id).await?}),
    ))
}

async fn workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        json!({"workspaces":state.store.workspaces(&access, query.project_id.as_deref()).await?}),
    ))
}
async fn workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let workspace = state.store.workspace(&access, &id).await?;
    let mut public = workspace.public();
    let can_edit = access
        .workspace_permission(&access.owner_id, &workspace.row["request_config"])?
        == crate::access::Permission::Edit;
    public["computerRecreationCanEdit"] = json!(can_edit);
    public["computerRecreationAvailable"] = json!(if can_edit {
        if let Some(runtime) = &state.runtime {
            runtime.recreation_available(&workspace.row).await?
        } else {
            false
        }
    } else {
        false
    });
    public["runtimeRepairCanEdit"] = json!(can_edit);
    if crate::runtime::task_commands::command_mode(&workspace.row) {
        public["taskCommandCanEdit"] = json!(can_edit);
    }
    if !can_edit {
        if public["allocationRecovery"].is_object() {
            public["allocationRecovery"]["retryAvailable"] = json!(false);
        }
        public["runtimeRepairAvailable"] = json!(false);
    }
    Ok(Json(public))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskHistoryQuery {
    before_sequence: Option<i64>,
    cache_projection: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolOutputQuery {
    blob_ref: String,
    offset: usize,
    limit: usize,
}
async fn tool_output_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ToolOutputQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        runtime(&state)?
            .tool_output_read(&access, &id, &query.blob_ref, query.offset, query.limit)
            .await?,
    ))
}
async fn tool_output_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(chunk): Json<zuse_cloud_runtime::tool_output::ToolOutputChunk>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        runtime(&state)?
            .tool_output_upload(
                &id,
                runtime_bearer(&headers)?,
                chunk,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn preparation_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(runtime(&state)?.preparation_logs(&access, &id).await?))
}
async fn task_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, session)): Path<(String, String)>,
    Query(query): Query<TaskHistoryQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        runtime(&state)?
            .task_history_cached(
                &access,
                &id,
                &session,
                query.before_sequence,
                query.cache_projection == Some(true),
            )
            .await?,
    ))
}
async fn archive_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let workspace = state.store.workspace(&access, &id).await?;
    Ok(Json(
        serde_json::to_value(archive::status(&workspace.row))
            .map_err(|_| ApiError::unavailable("archive_status_invalid"))?,
    ))
}

async fn command_enqueue(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_commands::CommandEnvelope>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    state.store.workspace(&access, &id).await?;
    let Json(command) = body.map_err(|_| ApiError::bad_request("cloud_command_not_eligible"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            runtime
                .command_enqueue(&id, &access, command, chrono::Utc::now().timestamp_millis())
                .await?,
        ),
    ))
}
async fn command_status(
    State(state): State<AppState>,
    Path((id, command)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    command_item(state, id, command, headers, false).await
}
async fn command_cancel(
    State(state): State<AppState>,
    Path((id, command)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    command_item(state, id, command, headers, true).await
}
async fn command_item(
    state: AppState,
    id: String,
    command: String,
    headers: HeaderMap,
    cancel: bool,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    state.store.workspace(&access, &id).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .command_status(
                &id,
                &access,
                &command,
                cancel,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandWatchQuery {
    #[serde(default)]
    after_revision: i64,
    wait_ms: Option<u32>,
}
async fn command_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<CommandWatchQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    state.store.workspace(&access, &id).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    let wait = query
        .wait_ms
        .unwrap_or(zuse_protocol::cloud_commands::WATCH_MAX_WAIT_MS);
    if wait > zuse_protocol::cloud_commands::WATCH_MAX_WAIT_MS {
        return Err(ApiError::bad_request("invalid_mailbox_wait"));
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait.into());
    let mut events = runtime.mailbox_changes.subscribe();
    loop {
        let page = runtime
            .command_watch(&id, &access, query.after_revision)
            .await?;
        if !page["changes"].as_array().is_some_and(|v| v.is_empty())
            || page["resetRequired"] == true
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(Json(page));
        }
        tokio::select! { _=events.recv()=>{}, _=tokio::time::sleep_until(deadline.min(tokio::time::Instant::now()+std::time::Duration::from_secs(1)))=>{} }
    }
}
async fn command_data_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    state.store.workspace(&access, &id).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(runtime.command_data_key(&id, &access).await?))
}
async fn command_lease(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_commands::LeaseRequest>, JsonRejection>,
) -> Result<Json<zuse_protocol::cloud_commands::LeasePage>, ApiError> {
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("cloud_mailbox_revision_invalid"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    let credential = runtime_bearer(&headers)?;
    let wait = request.wait_ms.unwrap_or(0);
    if wait > zuse_protocol::cloud_commands::WATCH_MAX_WAIT_MS {
        return Err(ApiError::bad_request("invalid_mailbox_wait"));
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(wait.into());
    let mut events = runtime.mailbox_changes.subscribe();
    loop {
        let page = runtime
            .command_lease(
                &id,
                credential,
                &request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        if !page.leases.is_empty()
            || page.fence_required
            || request
                .after_revision
                .is_some_and(|v| v != page.mailbox_revision)
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(Json(page));
        }
        tokio::select! {_=events.recv()=>{},_=tokio::time::sleep_until(deadline.min(tokio::time::Instant::now()+std::time::Duration::from_secs(1)))=>{}}
    }
}
async fn command_ack(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_commands::RuntimeAcknowledgment>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_runtime_command_ack"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .command_ack(
                &id,
                runtime_bearer(&headers)?,
                &request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

async fn workspace_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<crate::runtime::creation::CreateWorkspace>, JsonRejection>,
) -> Result<Response, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_cloud_workspace_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    let (response, created) = runtime
        .create_workspace(&access, body, chrono::Utc::now().timestamp_millis())
        .await?;
    let id = response["workspace"]["workspaceId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let mut response = (
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(response),
    )
        .into_response();
    if created {
        response.headers_mut().insert(
            "x-zuse-reconcile-cloud-workspace",
            HeaderValue::from_str(&id)
                .map_err(|_| ApiError::unavailable("cloud_workspace_response_invalid"))?,
        );
    }
    Ok(response)
}

async fn provider_grant(
    State(state): State<AppState>,
    Path((id, provider)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_auth::ProviderGrantRequest>, JsonRejection>,
) -> Result<Json<zuse_protocol::cloud_auth::SealedProviderGrant>, ApiError> {
    let provider = serde_json::from_value(serde_json::json!(provider))
        .map_err(|_| ApiError::not_found("cloud_provider_not_found"))?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_provider_grant_request"))?;
    let credential = runtime_bearer(&headers)?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .provider_grant(
                &id,
                credential,
                provider,
                body,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

async fn workspace_action(
    state: AppState,
    headers: HeaderMap,
    request: zuse_protocol::cloud_lifecycle::ActionRequest,
    action: zuse_protocol::cloud_lifecycle::Action,
) -> Result<Response, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    state
        .store
        .workspace(&access, &request.workspace_id)
        .await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    let workspace = runtime
        .lifecycle_action(
            &request.workspace_id,
            &access,
            action,
            request.command_id.as_deref(),
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    let mut response = Json(workspace).into_response();
    response.headers_mut().insert(
        "x-zuse-reconcile-cloud-workspace",
        HeaderValue::from_str(&request.workspace_id)
            .map_err(|_| ApiError::bad_request("invalid_workspace_id"))?,
    );
    Ok(response)
}
macro_rules! lifecycle_action_route {
    ($name:ident, $action:ident) => {
        async fn $name(
            State(state): State<AppState>,
            Path(id): Path<String>,
            headers: HeaderMap,
            body: Result<Json<zuse_protocol::cloud_lifecycle::ActionRequest>, JsonRejection>,
        ) -> Result<Response, ApiError> {
            let Json(request) =
                body.map_err(|_| ApiError::bad_request("invalid_cloud_workspace_action"))?;
            if id != request.workspace_id {
                return Err(ApiError::bad_request("workspace_id_mismatch"));
            }
            workspace_action(
                state,
                headers,
                request,
                zuse_protocol::cloud_lifecycle::Action::$action,
            )
            .await
        }
    };
}
lifecycle_action_route!(workspace_pause, Pause);
lifecycle_action_route!(workspace_archive, Archive);
lifecycle_action_route!(workspace_unarchive, Unarchive);
lifecycle_action_route!(workspace_resume, Resume);
async fn runtime_lifecycle(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_lifecycle::RuntimeRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("invalid_runtime_lifecycle_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .lifecycle_runtime(
                &id,
                runtime_bearer(&headers)?,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

async fn transcript_get(
    state: AppState,
    id: String,
    session: String,
    headers: HeaderMap,
    query: crate::transcript::ReadQuery,
    page: bool,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        state
            .store
            .transcript_read(&access, &id, &session, query, page)
            .await?,
    ))
}
macro_rules! transcript_read_route {
    ($name:ident,$page:literal) => {
        async fn $name(
            State(state): State<AppState>,
            Path((id, session)): Path<(String, String)>,
            headers: HeaderMap,
            Query(query): Query<crate::transcript::ReadQuery>,
        ) -> Result<Json<Value>, ApiError> {
            transcript_get(state, id, session, headers, query, $page).await
        }
    };
}
transcript_read_route!(transcript_checkpoint, false);
transcript_read_route!(transcript_page, true);
macro_rules! transcript_write_route {
    ($name:ident,$page:literal) => {
        async fn $name(
            State(state): State<AppState>,
            Path(id): Path<String>,
            headers: HeaderMap,
            body: Result<Json<crate::transcript::Upload>, JsonRejection>,
        ) -> Result<Json<Value>, ApiError> {
            let Json(body) =
                body.map_err(|_| ApiError::bad_request("invalid_transcript_upload"))?;
            let control = state
                .runtime
                .as_ref()
                .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
            Ok(Json(
                control
                    .transcript_upload(
                        &id,
                        runtime_bearer(&headers)?,
                        body,
                        $page,
                        chrono::Utc::now().timestamp_millis(),
                    )
                    .await?,
            ))
        }
    };
}
transcript_write_route!(transcript_upload, false);
transcript_write_route!(transcript_page_upload, true);

async fn archive_object_get(
    State(state): State<AppState>,
    Path((id, operation, object)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("workspace_archive_capability_invalid"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    let bytes = runtime
        .archive_object(
            &id,
            &operation,
            &object,
            token,
            None,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?
        .ok_or_else(|| ApiError::not_found("workspace_archive_object_missing"))?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}
async fn archive_object_put(
    State(state): State<AppState>,
    Path((id, operation, object)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("workspace_archive_capability_invalid"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    runtime
        .archive_object(
            &id,
            &operation,
            &object,
            token,
            Some(body.to_vec()),
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn workspace_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_lifecycle::ActionRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_cloud_workspace_action"))?;
    if body.workspace_id != id {
        return Err(ApiError::bad_request("workspace_id_mismatch"));
    }
    let access = state.access(&headers, AccessKind::Content).await?;
    state.store.workspace(&access, &id).await?;
    let control = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    let value = control
        .delete_workspace(
            &id,
            &access,
            body.command_id.as_deref(),
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok(([("x-zuse-reconcile-cloud-workspace", id)], Json(value)).into_response())
}

async fn billing_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<zuse_protocol::cloud_billing::BillingSummary>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    Ok(Json(
        state
            .store
            .billing_summary(&access.owner_id, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
#[derive(Deserialize)]
struct BillingUsageQuery {
    cursor: Option<String>,
    limit: Option<f64>,
}
async fn billing_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BillingUsageQuery>,
) -> Result<Json<zuse_protocol::cloud_billing::BillingUsagePage>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    let limit = query.limit.unwrap_or(50.);
    if !limit.is_finite() {
        return Err(ApiError::bad_request("invalid_billing_limit"));
    }
    Ok(Json(
        state
            .store
            .billing_usage(
                &access.owner_id,
                query.cursor.as_deref(),
                limit.trunc().clamp(1., 100.) as usize,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn billing_cap(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_billing::BillingCapRequest>, JsonRejection>,
) -> Result<Json<zuse_protocol::cloud_billing::BillingSummary>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_billing_cap"))?;
    Ok(Json(
        state
            .store
            .billing_set_cap(
                &access.owner_id,
                body,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

async fn billing_portal(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    Ok(Json(
        json!({"portalUrl":state.store.billing_portal(&access.owner_id).await?}),
    ))
}

async fn billing_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .store
            .billing_webhook(None, &headers, &body, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn billing_webhook_provider(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        state
            .store
            .billing_webhook(
                Some(&provider),
                &headers,
                &body,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

async fn billing_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_billing::BillingCheckoutRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_machine_offer"))?;
    Ok(Json(
        json!({"checkoutUrl":state.store.billing_checkout(&access.owner_id,&body.offer_id,chrono::Utc::now().timestamp_millis()).await?}),
    ))
}
#[derive(Deserialize)]
struct CheckoutReturn {
    t: Option<String>,
    checkout_id: Option<String>,
}
async fn checkout_complete(
    State(state): State<AppState>,
    Query(query): Query<CheckoutReturn>,
) -> Response {
    let summary = state
        .store
        .checkout_receipt(
            query.t.as_deref(),
            query.checkout_id.as_deref(),
            chrono::Utc::now().timestamp_millis(),
        )
        .await;
    let (title, detail) = match summary {
        Some(summary) if summary.status == "paid" => (
            "Payment confirmed",
            format!(
                "{} {}.{:02}. Return to Zuse and refresh Cloud to see your subscription.",
                summary.currency.to_uppercase(),
                summary.amount_cents / 100,
                summary.amount_cents % 100
            ),
        ),
        Some(summary) if summary.status == "failed" => (
            "Checkout was not completed",
            "Return to Zuse to start checkout again.".into(),
        ),
        _ => (
            "Payment confirmation pending",
            "Return to Zuse and refresh Cloud once your payment is confirmed.".into(),
        ),
    };
    ([ (header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CACHE_CONTROL,"no-store"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'"),(header::REFERRER_POLICY,"no-referrer") ],format!("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width, initial-scale=1'><title>Zuse checkout</title><body style='font-family:system-ui;max-width:480px;margin:80px auto;padding:24px'><h1>{title}</h1><p>{detail}</p></body></html>")).into_response()
}

async fn prepaid_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<zuse_protocol::cloud_billing::PrepaidBalance>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    Ok(Json(state.store.prepaid_balance(&access.owner_id).await?))
}
async fn prepaid_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_billing::PrepaidCheckoutRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Billing).await?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_prepaid_amount"))?;
    Ok(Json(
        json!({"checkoutUrl":state.store.prepaid_checkout(&access.owner_id,body.amount_cents,chrono::Utc::now().timestamp_millis()).await?}),
    ))
}
async fn prepaid_complete() -> Response {
    ([(header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CACHE_CONTROL,"no-store"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'"),(header::REFERRER_POLICY,"no-referrer")],"<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width, initial-scale=1'><title>Zuse credits</title><body style='font-family:system-ui;max-width:480px;margin:80px auto;padding:24px'><h1>Thanks for your purchase</h1><p>Your balance will update once payment is confirmed. Return to Zuse and refresh Cloud to see your credits.</p><p>Credits apply automatically to future Zuse invoices, including subscriptions.</p></body></html>").into_response()
}

async fn project_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<zuse_protocol::cloud_projects::ProjectList>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(state.store.project_list(&access.owner_id).await?))
}
fn project_create_precondition(headers: &HeaderMap) -> Result<bool, ApiError> {
    let values: Vec<_> = headers.get_all(header::IF_NONE_MATCH).iter().collect();
    if values.is_empty() {
        return Ok(false);
    }
    if values.len() == 1 && values[0].to_str().ok().map(str::trim) == Some("*") {
        return Ok(true);
    }
    Err(ApiError::bad_request(
        "cloud_project_create_precondition_invalid",
    ))
}
async fn project_connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_projects::ProjectConnectRequest>, JsonRejection>,
) -> Result<
    (
        StatusCode,
        Json<zuse_protocol::cloud_projects::CloudProject>,
    ),
    ApiError,
> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_request"))?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .store
                .project_connect_conditionally(
                    &access.owner_id,
                    request,
                    chrono::Utc::now().timestamp_millis(),
                    project_create_precondition(&headers)?,
                )
                .await?,
        ),
    ))
}
async fn project_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<zuse_protocol::cloud_projects::CloudProject>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    Ok(Json(
        state
            .store
            .project_remove(&access.owner_id, &id, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}

async fn github_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    if let Some(native) = state
        .store
        .github_native
        .as_ref()
        .filter(|g| g.enabled(&access.owner_id))
    {
        if native.connection_state(&access).await? == "connected" {
            if let Some(runtime) = state
                .runtime
                .as_ref()
                .filter(|r| r.scope_authority_configured())
            {
                runtime.scope_admission(&access).await?;
            }
        }
        let mut catalog = serde_json::to_value(native.catalog(&access).await?)
            .map_err(|_| ApiError::unavailable("cloud_github_unavailable"))?;
        catalog["nativeAuthority"] = json!(true);
        catalog["connectionState"] = json!(native.connection_state(&access).await?);
        return Ok(Json(catalog));
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .map(|(_, token)| token)
        .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
    let github = state
        .store
        .github
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_github_unavailable"))?;
    Ok(Json(
        serde_json::to_value(github.catalog(&access, bearer).await?)
            .map_err(|_| ApiError::unavailable("cloud_github_unavailable"))?,
    ))
}

async fn github_install(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    if let Some(native) = state
        .store
        .github_native
        .as_ref()
        .filter(|g| g.enabled(&access.owner_id))
    {
        return Ok(Json(native.begin(&access).await?));
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .map(|(_, token)| token)
        .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
    let github = state
        .store
        .github
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_github_unavailable"))?;
    Ok(Json(github.install(&access, bearer).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageQuery {
    provider_id: Option<String>,
}
async fn image_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ImageQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    if query.provider_id.as_deref().is_some_and(|p| p != "boxd") {
        return Err(ApiError::conflict("cloud_provider_unavailable"));
    }
    let logs = access
        .membership
        .as_ref()
        .is_none_or(|m| m.role.slug == "admin" || m.role.slug == "owner");
    Ok(Json(
        state.store.image_status(&access.owner_id, logs).await?,
    ))
}
async fn image_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_projects::ImageBuildRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_image_build_request"))?;
    let control = state
        .store
        .images
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_image_writes_disabled"))?;
    // Check writer ownership before calling the legacy authorization authority.
    control.owned(&access.owner_id).await?.rollback().await?;
    let private:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND included=true AND visibility='private')").bind(&access.owner_id).fetch_one(&control.pool).await?;
    let github = if private {
        if std::env::var("GITHUB_APP_ID")
            .unwrap_or_default()
            .is_empty()
            || std::env::var("GITHUB_APP_PRIVATE_KEY")
                .unwrap_or_default()
                .is_empty()
        {
            return Err(ApiError::unavailable("image_github_grants_not_configured"));
        }
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.split_once(' '))
            .map(|(_, token)| token)
            .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
        Some(
            state
                .store
                .github
                .as_ref()
                .ok_or_else(|| ApiError::unavailable("cloud_github_unavailable"))?
                .catalog(&access, bearer)
                .await?,
        )
    } else {
        None
    };
    Ok((
        StatusCode::ACCEPTED,
        Json(
            state
                .store
                .image_build(
                    &access,
                    request,
                    github,
                    chrono::Utc::now().timestamp_millis(),
                )
                .await?,
        ),
    ))
}

async fn image_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_projects::ImageDeleteRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_image_delete_request"))?;
    state
        .store
        .images
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_image_writes_disabled"))?
        .delete_image(
            &access.owner_id,
            &request.snapshot_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(state.store.image_status(&access.owner_id, true).await?),
    ))
}

async fn launch_asset_chunk(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_assets::Chunk>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_launch_asset"))?;
    Ok(Json(
        runtime(&state)?
            .launch_asset_chunk(&access, body, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn launch_asset_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<zuse_protocol::cloud_assets::Commit>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let Json(body) = body.map_err(|_| ApiError::bad_request("invalid_launch_asset"))?;
    Ok(Json(
        runtime(&state)?
            .launch_asset_commit(&access, body, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn runtime_asset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, asset)): Path<(String, String)>,
) -> Result<Json<zuse_protocol::cloud_assets::Download>, ApiError> {
    let credential = runtime_bearer(&headers)?;
    Ok(Json(
        runtime(&state)?
            .runtime_asset(
                &id,
                credential,
                &asset,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

fn runtime(state: &AppState) -> Result<&RuntimeControl, ApiError> {
    state
        .runtime
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))
}

#[derive(Deserialize)]
struct GithubStateQuery {
    state: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubOperationQuery {
    operation_id: String,
}
fn github_browser(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            (name == "__Host-zuse-github").then_some(value)
        })
}
fn native_github(state: &AppState) -> Result<&crate::github::native::NativeGithub, ApiError> {
    state
        .store
        .github_native
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("github_native_disabled"))
}
async fn github_connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GithubStateQuery>,
) -> Result<Response, ApiError> {
    let (url, browser) = native_github(&state)?
        .browser_start(&query.state, github_browser(&headers))
        .await?;
    let mut response = axum::response::Redirect::temporary(&url).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "__Host-zuse-github={browser}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=600"
        ))
        .map_err(|_| ApiError::unavailable("github_cookie_unavailable"))?,
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}
async fn github_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<crate::github::native::Callback>,
) -> Result<Response, ApiError> {
    native_github(&state)?
        .callback(&query, github_browser(&headers))
        .await?;
    Ok(github_browser_response(
        native_github(&state)?
            .browser_page(&query.state, github_browser(&headers))
            .await?,
    ))
}
fn github_browser_response(html: String) -> Response {
    let mut response = axum::response::Html(html).into_response();
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'"));
    // no-referrer serializes Origin as null on native form POSTs. Keep the
    // exact origin check while excluding OAuth codes/state paths from referrers.
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin"),
    );
    response
}
async fn github_browser_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GithubStateQuery>,
) -> Result<Response, ApiError> {
    Ok(github_browser_response(
        native_github(&state)?
            .browser_page(&query.state, github_browser(&headers))
            .await?,
    ))
}
async fn github_browser_access(
    state: &AppState,
    transaction: crate::github::native::BrowserTransaction,
) -> Result<Access, ApiError> {
    let actor = crate::auth::Principal {
        account_id: transaction.actor,
    };
    let (scope, membership) = if transaction.owner == actor.account_id {
        (access::Scope::Personal, None)
    } else {
        let organization_id = transaction
            .owner
            .strip_prefix("organization:")
            .ok_or_else(|| ApiError::forbidden("github_browser_scope_mismatch"))?;
        if !state.organizations_enabled
            || !state
                .organization_ids
                .iter()
                .any(|id| id == organization_id)
        {
            return Err(ApiError::forbidden("organization_workspaces_disabled"));
        }
        let member = state
            .identity
            .membership(&actor.account_id, organization_id)
            .await?;
        if member.status != "active"
            || member.user_id != actor.account_id
            || member.organization_id != organization_id
            || !matches!(member.role.slug.as_str(), "admin" | "member")
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        (
            access::Scope::Organization {
                organization_id: organization_id.to_owned(),
            },
            Some(member),
        )
    };
    Ok(Access {
        actor,
        scope,
        owner_id: transaction.owner,
        membership,
    })
}
async fn github_browser_select(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(selection): axum::Form<crate::github::native::BrowserSelection>,
) -> Result<Response, ApiError> {
    let github = native_github(&state)?;
    if !headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| github.browser_origin_matches(origin))
    {
        return Err(ApiError::forbidden("github_browser_origin_mismatch"));
    }
    let transaction = github
        .confirm_browser(&selection, github_browser(&headers))
        .await?;
    let operation_id = transaction.operation_id.clone();
    let access = github_browser_access(&state, transaction).await?;
    if let Err(error) = github
        .select(
            &access,
            &crate::github::native::Selection {
                operation_id,
                installation_id: selection.installation_id,
            },
        )
        .await
    {
        let mut response =
            github_browser_response(github.browser_failure_page(&selection.state, error.code));
        *response.status_mut() = error.status;
        return Ok(response);
    }
    if let Some(joining) = state.store.github_organization_authority() {
        joining.notify_dispatch();
    }
    Ok(github_browser_response(
        github
            .browser_page(&selection.state, github_browser(&headers))
            .await?,
    ))
}
async fn github_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GithubOperationQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    Ok(Json(
        native_github(&state)?
            .operation(&access, &query.operation_id)
            .await?,
    ))
}
async fn github_select(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(selection): Json<crate::github::native::Selection>,
) -> Result<StatusCode, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    native_github(&state)?.select(&access, &selection).await?;
    if let Some(joining) = state.store.github_organization_authority() {
        joining.notify_dispatch();
    }
    Ok(StatusCode::NO_CONTENT)
}
async fn github_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(query): Json<GithubOperationQuery>,
) -> Result<StatusCode, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    native_github(&state)?
        .cancel(&access, &query.operation_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GithubInstallationRemoval {
    installation_id: i64,
}
async fn github_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<GithubInstallationRemoval>,
) -> Result<Json<Value>, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    Ok(Json(
        native_github(&state)?
            .remove_installation(&access, request.installation_id)
            .await?,
    ))
}
async fn github_disconnect(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    native_github(&state)?.disconnect(&access).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn github_webhook_admission(
    State(gate): State<Arc<tokio::sync::Semaphore>>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    // A busy receiver rejects without queueing or polling another request body.
    // Hold the single buffer permit through signature validation and handling.
    let Ok(_permit) = gate.try_acquire_owned() else {
        return ApiError::new(StatusCode::TOO_MANY_REQUESTS, "github_webhook_busy").into_response();
    };
    next.run(request).await
}

async fn github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    };
    native_github(&state)?
        .webhook(
            get("x-hub-signature-256"),
            get("x-github-delivery"),
            get("x-github-event"),
            &body,
        )
        .await?;
    if let Some(joining) = state.store.github_organization_authority() {
        joining.notify_dispatch();
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn runtime_github_grant(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<crate::runtime::github::GitGrantRequest>,
) -> Result<Json<Value>, ApiError> {
    let credential = runtime_bearer(&headers)?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .github_grant(
                native_github(&state)?,
                &id,
                credential,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
#[derive(Deserialize)]
struct GithubPullRequest {
    #[serde(default)]
    draft: bool,
    repository: String,
    title: String,
    head: String,
    base: String,
    #[serde(default)]
    body: String,
}
async fn github_pull_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<GithubPullRequest>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        native_github(&state)?
            .create_pull_request(
                &access,
                &request.repository,
                &request.title,
                &request.head,
                &request.base,
                &request.body,
                request.draft,
            )
            .await?,
    ))
}

async fn allocation_scope(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    runtime
        .authorize_allocation_scope(
            &id,
            runtime_bearer(&headers)?,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_workload_secrets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(runtime.workload_secrets(&access, &project).await?))
}
async fn save_workload_secret(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    body: Result<Json<crate::runtime::workload_secrets::SaveWorkloadSecret>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let Json(request) = body.map_err(|_| ApiError::bad_request("invalid_workload_secret"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .save_workload_secret(
                &access,
                &project,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecretRevision {
    expected_revision: i64,
}
async fn revoke_workload_secret(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, secret)): Path<(String, String)>,
    Query(revision): Query<SecretRevision>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .revoke_workload_secret(
                &access,
                &project,
                &secret,
                revision.expected_revision,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn workload_secret_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<crate::runtime::workload_secrets::WorkloadGrantRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let credential = runtime_bearer(&headers)?;
    let Json(request) =
        body.map_err(|_| ApiError::bad_request("invalid_workload_grant_request"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .workload_grant(
                &id,
                credential,
                request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

async fn github_browser_identity(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(query): axum::Form<GithubStateQuery>,
) -> Result<Response, ApiError> {
    let github = native_github(&state)?;
    if !headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| github.browser_origin_matches(origin))
    {
        return Err(ApiError::forbidden("github_browser_origin_mismatch"));
    }
    let transaction = github
        .confirm_identity_browser(&query.state, github_browser(&headers))
        .await?;
    let operation = transaction.operation_id.clone();
    let access = github_browser_access(&state, transaction).await?;
    github.finalize_identity(&access, &operation).await?;
    if let Some(joining) = state.store.github_organization_authority() {
        joining.notify_dispatch();
    }
    Ok(github_browser_response(
        github
            .browser_page(&query.state, github_browser(&headers))
            .await?,
    ))
}

async fn worker_effect_scope(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    runtime
        .authorize_worker_effect_scope(
            &id,
            runtime_bearer(&headers)?,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn github_setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<crate::github::native::organizations::SetupCallback>,
) -> Result<Response, ApiError> {
    if let Some(legacy) = &state.store.github {
        if let Some(url) =
            crate::github::native::organizations::legacy_setup_forward(legacy, &query)?
        {
            return Ok(axum::response::Redirect::temporary(url.as_str()).into_response());
        }
    }
    Ok(github_browser_response(
        if let Some(github) = &state.store.github_native {
            github.setup_result(&query, github_browser(&headers)).await
        } else {
            crate::github::native::organizations::setup_notice()
        },
    ))
}

async fn runtime_repair_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<crate::runtime::repair::Start>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .restart_runtime(&id, &access, body, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
async fn runtime_repair_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, operation)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .runtime_repair_status(&id, &operation, &access)
            .await?,
    ))
}
async fn runtime_repair_resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, operation)): Path<(String, String)>,
    Json(body): Json<crate::runtime::repair::Resolve>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .resolve_runtime_repair(
                &id,
                &operation,
                &access,
                body,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
async fn runtime_repair_retry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, operation)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .retry_runtime_repair(
                &id,
                &operation,
                &access,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimePreparationIngress {
    operation_id: String,
}
async fn runtime_preparation_ingress(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<RuntimePreparationIngress>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(body) =
        body.map_err(|_| ApiError::bad_request("invalid_runtime_preparation_ingress"))?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_disabled"))?;
    Ok(Json(
        runtime
            .preparation_ingress(
                &id,
                runtime_bearer(&headers)?,
                &body.operation_id,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}

#[cfg(test)]
mod github_pull_request_contract {
    use super::*;
    #[test]
    fn draft_is_explicit_and_legacy_requests_keep_their_default() {
        let input = json!({"repository":"team/project","title":"Fixture","head":"test/proof","base":"main"});
        let default: GithubPullRequest = serde_json::from_value(input.clone()).unwrap();
        assert!(!default.draft);
        let mut draft = input;
        draft["draft"] = json!(true);
        assert!(
            serde_json::from_value::<GithubPullRequest>(draft.clone())
                .unwrap()
                .draft
        );
        draft["draft"] = json!("true");
        assert!(serde_json::from_value::<GithubPullRequest>(draft).is_err());
    }
}

async fn allocation_retry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, operation)): Path<(String, String)>,
    Json(body): Json<crate::runtime::allocation::rejection::Retry>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .retry_allocation(
                &id,
                &operation,
                &access,
                body,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
