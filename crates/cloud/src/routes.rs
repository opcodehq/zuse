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
    extract::{DefaultBodyLimit, Path, Query, State, WebSocketUpgrade, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, Method, header},
    middleware,
    response::Response,
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
        .allow_methods(if state.runtime.is_some() {
            vec![Method::GET, Method::POST]
        } else {
            vec![Method::GET]
        })
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
        .route("/v1/billing/entitlements", get(entitlements))
        .route("/v1/cloud/workspaces", get(workspaces))
        .route("/v1/cloud/workspaces/{id}", get(workspace))
        .route(
            "/v1/cloud/workspaces/{id}/archive-status",
            get(archive_status),
        );
    if state.runtime.is_some() {
        app = app
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
    Ok(Json(
        json!({"actor":access.actor,"scope":access.scope,"ownerId":access.owner_id}),
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
