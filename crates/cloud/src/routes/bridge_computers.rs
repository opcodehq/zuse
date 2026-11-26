//! Owner catalog. Enrolling a public key does not create a remote-access grant.
use super::{bridge_approvals::personal_owner, *};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Enrollment {
    label: String,
    platform: String,
}

pub(super) async fn enroll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Enrollment>,
) -> Result<Json<Value>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let label = input.label.trim();
    if label.is_empty()
        || label.len() > 128
        || label.chars().any(char::is_control)
        || !matches!(input.platform.as_str(), "macos" | "linux" | "windows")
    {
        return Err(ApiError::bad_request("invalid_bridge_computer"));
    }
    let key = host_key(&state, &headers, "/v1/cloud/bridge/computers").await?;
    let id: String = sqlx::query_scalar(
        "INSERT INTO api_bridge_computers(owner_id,enrollment_id,key_thumbprint,label,platform)
        VALUES ($1,$2,$3,$4,$5) ON CONFLICT(owner_id,key_thumbprint)
        DO UPDATE SET label=excluded.label,platform=excluded.platform RETURNING enrollment_id",
    )
    .bind(owner)
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(key)
    .bind(label)
    .bind(input.platform)
    .fetch_one(&runtime(&state)?.shared_pool())
    .await?;
    Ok(Json(json!({"deviceId":id})))
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let rows = sqlx::query_scalar(
        "SELECT jsonb_build_object('deviceId',enrollment_id,'name',label,'platform',platform,
        'connected',EXISTS(SELECT FROM api_bridge_tasks t WHERE t.owner_id=c.owner_id AND t.enrollment_id=c.enrollment_id AND t.expires_at>now()),
        'alwaysAllowed',EXISTS(SELECT FROM api_bridge_always_grants g WHERE g.owner_id=c.owner_id AND g.enrollment_id=c.enrollment_id))
        FROM api_bridge_computers c WHERE owner_id=$1 ORDER BY lower(label),enrollment_id",
    )
    .bind(owner)
    .fetch_all(&runtime(&state)?.shared_pool())
    .await?;
    Ok(Json(rows))
}

pub(super) async fn attached(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Vec<String>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let ids = sqlx::query_scalar(
        "SELECT enrollment_id FROM api_bridge_attachments
        WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 ORDER BY enrollment_id",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_all(&runtime(&state)?.shared_pool())
    .await?;
    Ok(Json(ids))
}

pub(super) async fn attach(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let pool = runtime(&state)?.shared_pool();
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT 1 FROM api_cloud_workspaces w JOIN zuse_rust_task_command_sessions s USING(workspace_id)
        JOIN api_bridge_computers c ON c.owner_id=w.account_id
        WHERE w.account_id=$1 AND workspace_id=$2 AND s.session_id=$3 AND c.enrollment_id=$4
        AND s.state='active' AND w.desired_state IN ('ready','paused')
        AND w.state NOT IN ('archived','archiving','deleting','deleted') FOR SHARE OF w,s,c")
        .bind(&owner).bind(&workspace).bind(&chat).bind(&id).fetch_optional(&mut *tx).await?
        .ok_or_else(||ApiError::not_found("bridge_computer_or_chat_unavailable"))?;
    sqlx::query(
        "INSERT INTO api_bridge_attachments(owner_id,workspace_id,chat_id,enrollment_id)
        VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn forget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    sqlx::query("DELETE FROM api_bridge_computers WHERE owner_id=$1 AND enrollment_id=$2")
        .bind(owner)
        .bind(id)
        .execute(&runtime(&state)?.shared_pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn host_key(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
) -> Result<String, ApiError> {
    let authority = state
        .store
        .devices
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("device_authority_disabled"))?;
    let token = headers[header::AUTHORIZATION]
        .to_str()
        .ok()
        .and_then(|v| v.split_once(' '))
        .map(|(_, t)| t)
        .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
    let proof = headers
        .get("dpop")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing_dpop"))?;
    authority.verify_bridge_proof(path, token, proof).await
}
