//! Authenticated API command plane. No handler acquires a runtime connection.
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueueQuery {
    session_id: String,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionsQuery {
    after_created_at: Option<i64>,
    after_session_id: Option<String>,
}
#[derive(Default, Deserialize)]
pub(super) struct AssetQuery {
    offset: Option<usize>,
}

pub(super) async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<zuse_protocol::cloud_task_commands::Request>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let session = request.session_id.clone();
    let response = runtime(&state)?
        .task_command_submit(&id, &access, request, chrono::Utc::now().timestamp_millis())
        .await?;
    if let Some(control) = &state.runtime {
        control.dispatch_task_drain(&id, Some(&session));
    }
    Ok(Json(response))
}
pub(super) async fn queue(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<QueueQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        runtime(&state)?
            .task_command_queue(&id, &access, &query.session_id)
            .await?,
    ))
}
pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, command)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        runtime(&state)?
            .task_command_status(&id, &access, &command)
            .await?,
    ))
}
pub(super) async fn sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<SessionsQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let after = match (query.after_created_at, query.after_session_id) {
        (None, None) => None,
        (Some(at), Some(id)) => Some((at, id)),
        _ => return Err(ApiError::bad_request("cloud_task_session_cursor_invalid")),
    };
    Ok(Json(
        runtime(&state)?
            .task_command_sessions(&id, &access, after)
            .await?,
    ))
}
pub(super) async fn progress(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(report): Json<zuse_protocol::cloud_task_commands::Progress>,
) -> Result<Json<Value>, ApiError> {
    let response = runtime(&state)?
        .task_command_progress(
            &id,
            runtime_bearer(&headers)?,
            &report,
            chrono::Utc::now().timestamp_millis(),
        )
        .await?;
    if let Some(control) = &state.runtime {
        control.dispatch_task_drain(&id, None);
    }
    Ok(Json(response))
}
pub(super) async fn asset_chunk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(chunk): Json<zuse_protocol::cloud_assets::Chunk>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        runtime(&state)?
            .workspace_asset_chunk(&id, &access, chunk, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
pub(super) async fn asset_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(commit): Json<zuse_protocol::cloud_assets::Commit>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        runtime(&state)?
            .workspace_asset_commit(&id, &access, commit, chrono::Utc::now().timestamp_millis())
            .await?,
    ))
}
pub(super) async fn asset_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, asset)): Path<(String, String)>,
    Query(query): Query<AssetQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        runtime(&state)?
            .workspace_asset_read(&id, &access, &asset, query.offset.unwrap_or(0))
            .await?,
    ))
}
