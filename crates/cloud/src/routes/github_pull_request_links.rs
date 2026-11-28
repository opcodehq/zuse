//! Pull requests owned by chats: claim, release, list, event settings, and the
//! inbox the hosting engine reads when a `pull-request` hint arrives.
use super::*;
use zuse_protocol::pull_request_links::{ClaimRequest, EventPolicy};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Release {
    chat_key: String,
    repository: String,
    number: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Policy {
    chat_key: String,
    repository: String,
    number: u64,
    policy: EventPolicy,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    sequences: Vec<i64>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/cloud/github/pull-requests/owned", get(owned))
        .route("/v1/cloud/github/pull-requests/claim", post(claim))
        .route("/v1/cloud/github/pull-requests/release", post(release))
        .route("/v1/cloud/github/pull-requests/policy", post(policy))
        .route("/v1/cloud/github/pull-requests/inbox", get(inbox))
        .route("/v1/cloud/github/pull-requests/inbox/ack", post(ack))
        .route("/v1/cloud/github/pull-requests/mentions", get(mentions))
        .route(
            "/v1/cloud/github/pull-requests/mentions/ack",
            post(ack_mentions),
        )
        .layer(DefaultBodyLimit::max(8192))
}

async fn owned(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let owned = native_github(&state)?.owned_pull_requests(&access).await?;
    Ok(Json(json!({ "pullRequests": owned })))
}

async fn claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimRequest>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let owned = native_github(&state)?
        .claim_pull_request(&access, &request)
        .await?;
    Ok(Json(json!({ "pullRequest": owned })))
}

async fn release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Release>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    native_github(&state)?
        .release_pull_request(
            &access,
            &request.chat_key,
            &request.repository,
            request.number,
        )
        .await?;
    Ok(Json(json!({ "released": true })))
}

async fn policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Policy>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let owned = native_github(&state)?
        .set_pull_request_policy(
            &access,
            &request.chat_key,
            &request.repository,
            request.number,
            &request.policy,
        )
        .await?;
    Ok(Json(json!({ "pullRequest": owned })))
}

async fn inbox(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let events = native_github(&state)?.pull_request_inbox(&access).await?;
    Ok(Json(json!({ "events": events })))
}

async fn ack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Ack>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let acked = native_github(&state)?
        .ack_pull_request_inbox(&access, &request.sequences)
        .await?;
    Ok(Json(json!({ "acked": acked })))
}

async fn mentions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let mentions = native_github(&state)?
        .pull_request_mentions(&access)
        .await?;
    Ok(Json(json!({ "mentions": mentions })))
}

async fn ack_mentions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Ack>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let acked = native_github(&state)?
        .ack_pull_request_mentions(&access, &request.sequences)
        .await?;
    Ok(Json(json!({ "acked": acked })))
}
