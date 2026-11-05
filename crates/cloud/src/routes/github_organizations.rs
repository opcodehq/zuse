//! Original organization GitHub paths, backed by standalone authority. Mutating
//! organization policy still requires a fresh WorkOS admin membership.
use super::*;
use crate::github::native::organizations::Joining;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    organization_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolicyInput {
    organization_id: String,
    installation_id: i64,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberInput {
    organization_id: String,
    account_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Existing {
    installation_id: i64,
}
fn joining(state: &AppState) -> Result<&Joining, ApiError> {
    state
        .store
        .github_joining
        .as_deref()
        .ok_or_else(|| ApiError::unavailable("organizations_not_configured"))
}
async fn personal(state: &AppState, headers: &HeaderMap) -> Result<Access, ApiError> {
    let mut headers = headers.clone();
    headers.remove("x-zuse-workspace");
    state.setup_access(&headers, AccessKind::Content).await
}
async fn organization(
    state: &AppState,
    headers: &HeaderMap,
    org: &str,
) -> Result<Access, ApiError> {
    let scope = access::Scope::parse(Some(&format!("organization:{org}")))?;
    let mut scoped = headers.clone();
    scoped.remove("x-zuse-workspace");
    scoped.insert(
        "x-zuse-workspace",
        HeaderValue::from_str(&format!("organization:{org}"))
            .map_err(|_| ApiError::bad_request("invalid_workspace_scope"))?,
    );
    let access = state
        .setup_access(&scoped, AccessKind::Administration)
        .await?;
    if access.scope != scope {
        return Err(ApiError::forbidden("organization_access_denied"));
    }
    Ok(access)
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/organizations/github/connection", post(connection))
        .route("/v1/organizations/github/authorize", post(authorize))
        .route("/v1/organizations/github/callback", get(callback))
        .route("/v1/organizations/github/settings", post(settings))
        .route("/v1/organizations/github/policy", post(policy))
        .route("/v1/organizations/github/restore", post(restore))
        .route("/v1/organizations/github/remove", post(remove))
        .route("/v1/organizations/github/sync", post(sync))
        .route("/v1/cloud/github/existing", post(existing))
        .layer(DefaultBodyLimit::max(8192))
}
async fn connection(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = personal(&state, &headers).await?;
    let authority = native_github(&state)?;
    let status = authority.connection_state(&access).await?;
    let row: Option<Value> =
        sqlx::query_scalar("SELECT data FROM api_github_identities WHERE account_id=$1")
            .bind(&access.actor.account_id)
            .fetch_optional(&state.store.pool)
            .await?;
    Ok(Json(match (row, status.as_str()) {
        (Some(id), "connected") => json!({"connected":true,"login":id["login"]}),
        _ => json!({"connected":false}),
    }))
}
async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = personal(&state, &headers).await?;
    Ok(Json(native_github(&state)?.begin(&access).await?))
}
async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<crate::github::native::Callback>,
) -> Result<Response, ApiError> {
    if query.code.is_none() && query.error.is_none() {
        return super::github_connect(
            State(state),
            headers,
            Query(GithubStateQuery { state: query.state }),
        )
        .await;
    }
    super::github_callback(State(state), headers, Query(query)).await
}
async fn settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Result<Json<Value>, ApiError> {
    let access = organization(&state, &headers, &input.organization_id).await?;
    Ok(Json(joining(&state)?.settings(&access).await?))
}
async fn policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PolicyInput>,
) -> Result<Json<Value>, ApiError> {
    let access = organization(&state, &headers, &input.organization_id).await?;
    joining(&state)?
        .set_policy(&access, input.installation_id, input.enabled)
        .await?;
    if input.enabled {
        joining(&state)?
            .schedule(input.installation_id, chrono::Utc::now().timestamp_millis())
            .await?;
    }
    Ok(Json(json!({"ok":true})))
}
async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<MemberInput>,
) -> Result<Json<Value>, ApiError> {
    let access = organization(&state, &headers, &input.organization_id).await?;
    joining(&state)?.restore(&access, &input.account_id).await?;
    if let Some(runtime) = &state.runtime {
        runtime
            .invalidate_organization_access(&input.organization_id, &input.account_id)
            .await;
    }
    Ok(Json(json!({"ok":true})))
}
async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<MemberInput>,
) -> Result<Json<Value>, ApiError> {
    let access = organization(&state, &headers, &input.organization_id).await?;
    joining(&state)?.block(&access, &input.account_id).await?;
    if let Some(runtime) = &state.runtime {
        runtime
            .invalidate_organization_access(&input.organization_id, &input.account_id)
            .await;
    }
    Ok(Json(json!({"ok":true})))
}
async fn sync(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let access = personal(&state, &headers).await?;
    Ok(Json(
        json!({"joinedOrganizationIds":joining(&state)?.sync(&access.actor.account_id,chrono::Utc::now().timestamp_millis()).await?}),
    ))
}
async fn existing(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Existing>,
) -> Result<Json<Value>, ApiError> {
    let access = state.setup_access(&headers, AccessKind::Content).await?;
    let result = native_github(&state)?
        .connect_existing(&access, input.installation_id)
        .await?;
    if let Some(joining) = state.store.github_organization_authority() {
        joining.notify_dispatch();
    }
    Ok(Json(result))
}
