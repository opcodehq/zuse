//! Explicit computer replacement, independently authorized from runtime repair.
use super::*;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Start {
    idempotency_key: String,
}
pub(super) async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Start>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let runtime = state
        .runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("cloud_runtime_writes_disabled"))?;
    Ok(Json(
        runtime
            .recreate_computer(
                &id,
                &access,
                &body.idempotency_key,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
pub(super) async fn status(
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
        runtime.recreation_status(&id, &operation, &access).await?,
    ))
}
pub(super) async fn retry(
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
            .retry_recreation(
                &id,
                &operation,
                &access,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?,
    ))
}
