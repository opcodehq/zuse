use crate::{
    access::{self, Access, AccessKind},
    archive,
    auth::Identity,
    error::ApiError,
    store::Store,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, header},
    middleware,
    response::Response,
    routing::get,
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
        .allow_methods([Method::GET])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::HeaderName::from_static("x-zuse-workspace"),
        ]);
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/cloud/context", get(context))
        .route("/v1/billing/entitlements", get(entitlements))
        .route("/v1/cloud/workspaces", get(workspaces))
        .route("/v1/cloud/workspaces/{id}", get(workspace))
        .route(
            "/v1/cloud/workspaces/{id}/archive-status",
            get(archive_status),
        )
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

async fn health() -> Json<Value> {
    Json(
        json!({"status":"ok","service":"zuse-cloud-api","version":env!("CARGO_PKG_VERSION"),"mode":"read-only"}),
    )
}
async fn ready(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    state.store.ready().await?;
    Ok(Json(json!({"status":"ready","mode":"read-only"})))
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
