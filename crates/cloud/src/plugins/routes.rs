//! HTTP authorization boundary for the native plugin host.
use super::{PluginControl, PluginIdentity};
use crate::{
    access::{AccessKind, Scope},
    error::ApiError,
    routes::AppState,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde_json::Value;
use std::sync::Arc;
use zuse_protocol::plugins::{PluginRequest, PluginTenant, PluginToolRequest};

pub fn router(origins: Arc<Vec<axum::http::HeaderValue>>) -> Router<AppState> {
    Router::new()
        .route("/v1/organizations", get(organizations))
        .route("/v1/plugins", post(manage))
        .route("/v1/plugins/tools", post(tools))
        .route("/v1/plugins/callback", get(callback))
        .route("/v1/plugins/{tenant}/mcp", post(mcp))
        .route("/v1/plugins/runtime/{id}/tools", post(runtime_tools))
        .route(
            "/v1/organization-workspaces/{org}/v1/plugins",
            post(org_manage),
        )
        .route(
            "/v1/organization-workspaces/{org}/v1/plugins/tools",
            post(org_tools),
        )
        .layer(DefaultBodyLimit::max(128_000))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let origins = origins.clone();
                async move {
                    if request.method() == axum::http::Method::POST
                        && (request.headers().get_all("origin").iter().count() > 1
                            || request
                                .headers()
                                .get("origin")
                                .is_some_and(|origin| !origins.contains(origin)))
                    {
                        return ApiError::forbidden("plugin_origin_rejected").into_response();
                    }
                    next.run(request).await
                }
            },
        ))
}

async fn organizations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    // Discovery lists only this deployment's enabled organizations. Each entry
    // requires a fresh active membership; the list itself grants no authority.
    let mut personal = headers.clone();
    personal.insert(
        "x-zuse-workspace",
        "personal".parse().expect("static header"),
    );
    let access = state.access(&personal, AccessKind::Content).await?;
    let mut organizations = Vec::new();
    if state.organizations_enabled {
        for id in state.organization_ids.iter() {
            let member = match state
                .identity
                .membership(&access.actor.account_id, id)
                .await
            {
                Ok(member) => member,
                Err(error) if error.status == StatusCode::FORBIDDEN => continue,
                Err(error) => return Err(error),
            };
            if member.status == "active" && matches!(member.role.slug.as_str(), "admin" | "member")
            {
                let name = state.identity.organization_name(id).await?;
                organizations
                    .push(serde_json::json!({"id":id,"name":name,"role":member.role.slug}));
            }
        }
    }
    Ok(Json(Value::Array(organizations)))
}

fn control(state: &AppState) -> Result<&Arc<PluginControl>, ApiError> {
    state
        .store
        .plugins
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("plugins_unavailable"))
}

async fn identity(
    state: &AppState,
    headers: &HeaderMap,
    tenant: Option<&str>,
    kind: AccessKind,
) -> Result<(PluginIdentity, PluginTenant, bool), ApiError> {
    // Explicit tenant selection may supply the scope when there is no header.
    // A supplied header must agree with the body/path, never silently override it.
    let mut headers = headers.clone();
    if !headers.contains_key("x-zuse-workspace") {
        if let Some(tenant) = tenant.filter(|t| t.starts_with("organization:")) {
            headers.insert(
                "x-zuse-workspace",
                tenant
                    .parse()
                    .map_err(|_| ApiError::bad_request("invalid_workspace_scope"))?,
            );
        }
    }
    let access = state.access(&headers, kind).await?;
    let (id, kind, name) = match access.scope {
        Scope::Personal => (
            format!("personal:{}", access.actor.account_id),
            "personal",
            "Personal".to_owned(),
        ),
        Scope::Organization { .. } => (access.owner_id, "organization", "Organization".to_owned()),
    };
    if tenant.is_some_and(|tenant| tenant != id) {
        return Err(ApiError::forbidden("plugin_tenant_rejected"));
    }
    let can_manage = access
        .membership
        .as_ref()
        .is_none_or(|m| m.role.slug == "admin");
    Ok((
        PluginIdentity {
            tenant: id.clone(),
            subject: access.actor.account_id,
        },
        PluginTenant {
            id,
            kind: kind.into(),
            name: name.into(),
        },
        can_manage,
    ))
}

fn org_headers(mut headers: HeaderMap, org: &str) -> Result<HeaderMap, ApiError> {
    let scope = format!("organization:{org}");
    Scope::parse(Some(&scope))?;
    if headers.get_all("x-zuse-workspace").iter().count() > 1
        || headers
            .get("x-zuse-workspace")
            .is_some_and(|h| h.as_bytes() != scope.as_bytes())
    {
        return Err(ApiError::forbidden("plugin_tenant_rejected"));
    }
    headers.insert(
        "x-zuse-workspace",
        scope
            .parse()
            .map_err(|_| ApiError::bad_request("invalid_workspace_scope"))?,
    );
    Ok(headers)
}

async fn manage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PluginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let host = control(&state)?;
    let kind = if matches!(&input, PluginRequest::List { .. }) {
        AccessKind::Content
    } else {
        AccessKind::Administration
    };
    let (identity, mut tenant, can_manage) =
        identity(&state, &headers, input.tenant_id(), kind).await?;
    // Display metadata is optional. Tool execution depends on fresh membership,
    // never on the availability of an organization-name lookup.
    if matches!(&input, PluginRequest::List { .. }) && tenant.kind == "organization" {
        if let Some(id) = tenant.id.strip_prefix("organization:") {
            if let Ok(name) = state.identity.organization_name(id).await {
                tenant.name = name;
            }
        }
    }
    Ok(Json(
        host.request(&identity, tenant, can_manage, input).await?,
    ))
}
async fn org_manage(
    State(state): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
    input: Json<PluginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    manage(State(state), org_headers(headers, &org)?, input).await
}
async fn tools(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PluginToolRequest>,
) -> Result<Json<Value>, ApiError> {
    let host = control(&state)?;
    let (identity, _, _) = identity(&state, &headers, None, AccessKind::Content).await?;
    Ok(Json(host.tools(&identity, input).await?))
}
async fn org_tools(
    State(state): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
    input: Json<PluginToolRequest>,
) -> Result<Json<Value>, ApiError> {
    tools(State(state), org_headers(headers, &org)?, input).await
}
async fn callback(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let target = control(&state)?
        .callback(query.as_deref().unwrap_or_default())
        .await?;
    Ok((
        [(header::REFERRER_POLICY, "no-referrer")],
        Redirect::to(target.as_str()),
    )
        .into_response())
}
async fn mcp(
    State(state): State<AppState>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<Response, ApiError> {
    let host = control(&state)?;
    let (identity, _, _) = identity(&state, &headers, Some(&tenant), AccessKind::Content).await?;
    let forwarded: serde_json::Map<String, Value> = headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "content-type" | "accept" | "mcp-protocol-version" | "mcp-session-id"
            )
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), Value::String(value.into())))
        })
        .collect();
    let response = host
        .serve_mcp(&identity, input, Value::Object(forwarded))
        .await?;
    let status = StatusCode::from_u16(
        response["status"]
            .as_u64()
            .ok_or_else(|| ApiError::unavailable("executor_protocol_failed"))? as u16,
    )
    .map_err(|_| ApiError::unavailable("executor_protocol_failed"))?;
    Ok(if response["body"].is_null() {
        status.into_response()
    } else {
        (status, Json(response["body"].clone())).into_response()
    })
}
async fn runtime_tools(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<PluginToolRequest>,
) -> Result<Json<Value>, ApiError> {
    let host = control(&state)?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_unavailable"))?;
    let identity = runtime
        .plugin_identity(
            &id,
            crate::routes::runtime_bearer(&headers)?,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    Ok(Json(host.tools(&identity, input).await?))
}
