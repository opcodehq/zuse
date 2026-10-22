use crate::{
    access::{self, Access, AccessKind},
    archive,
    auth::Identity,
    error::ApiError,
    runtime::{BootstrapAck, BootstrapRequest, RuntimeControl},
    store::Store,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
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
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(if state.runtime.is_some() {
            vec![Method::GET, Method::POST]
        } else {
            vec![Method::GET]
        })
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("x-zuse-workspace"),
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
                "/v1/cloud/workspaces/{id}/runtime/bootstrap",
                post(runtime_bootstrap).layer(DefaultBodyLimit::max(64 * 1024)),
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
