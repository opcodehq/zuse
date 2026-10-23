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

pub fn router(state: AppState, origins: Vec<HeaderValue>) -> Router {
    let origins = Arc::new(origins);
    let gateway_origins = origins.clone();
    let cors = CorsLayer::new()
        .allow_origin(origins.as_ref().clone())
        .allow_methods(
            if state.runtime.is_some()
                || state.store.billing_writes_enabled()
                || state.store.project_writes_enabled()
            {
                vec![Method::GET, Method::POST, Method::PUT, Method::DELETE]
            } else {
                vec![Method::GET]
            },
        )
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("x-zuse-workspace"),
            header::HeaderName::from_static("x-zuse-device-id"),
        ]);
    let mut app = Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/cloud/context", get(context))
        .route("/v1/cloud/github", get(github_catalog))
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
    app.layer(middleware::map_response(private_response))
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
    let grant = control
        .gateway_authorize(
            &id,
            values[0],
            values[1],
            chrono::Utc::now().timestamp_millis(),
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
    let projects = state.store.launch_projects(&access.owner_id).await?;
    Ok(Json(
        json!({"actor":access.actor,"scope":access.scope,"ownerId":access.owner_id,"projects":projects}),
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
    Ok(Json(state.store.workspace(&access, &id).await?.public()))
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
                .project_connect(
                    &access.owner_id,
                    request,
                    chrono::Utc::now().timestamp_millis(),
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
) -> Result<Json<zuse_protocol::cloud_projects::GithubCatalog>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
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
    Ok(Json(github.catalog(&access, bearer).await?))
}
