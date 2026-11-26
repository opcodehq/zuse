//! Owner catalog, chat attachments and the chat's primary computer. Enrolling
//! a key grants nothing by itself: every command still needs approval.
use super::{bridge_approvals::personal_owner, bridge_proof::host_key, *};

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
    let pool = runtime(&state)?.shared_pool();
    let key = host_key(&pool, &headers, "/v1/cloud/bridge/computers").await?;
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
    .fetch_one(&pool)
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
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    Ok(Json(
        attached_for(&runtime(&state)?.shared_pool(), &owner, &workspace, &chat).await?,
    ))
}

/// The chat's primary (first, when there is one) and its explicitly attached
/// secondaries.
pub(super) async fn attached_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object('deviceId',a.enrollment_id,
            'role',CASE WHEN p.enrollment_id IS NULL THEN 'secondary' ELSE 'primary' END)
         FROM api_bridge_attachments a
         LEFT JOIN api_bridge_primaries p USING(owner_id,workspace_id,chat_id,enrollment_id)
         WHERE a.owner_id=$1 AND a.workspace_id=$2 AND a.chat_id=$3 AND (a.explicit OR p.enrollment_id IS NOT NULL)
         ORDER BY p.enrollment_id IS NULL, a.enrollment_id",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_all(pool)
    .await?)
}

/// The owner's active chat on a usable computer, and their enrollment, locked
/// against a concurrent archive or removal.
async fn usable_chat(
    tx: &mut sqlx::PgConnection,
    owner: &str,
    workspace: &str,
    chat: &str,
    device: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "SELECT 1 FROM api_cloud_workspaces w JOIN zuse_rust_task_command_sessions s USING(workspace_id)
         JOIN api_bridge_computers c ON c.owner_id=w.account_id
         WHERE w.account_id=$1 AND workspace_id=$2 AND s.session_id=$3 AND c.enrollment_id=$4
           AND s.state='active' AND w.desired_state IN ('ready','paused')
           AND w.state NOT IN ('archived','archiving','deleting','deleted') FOR SHARE OF w,s,c",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(device)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("bridge_computer_or_chat_unavailable"))?;
    Ok(())
}

pub(super) async fn attach(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    attach_for(
        &runtime(&state)?.shared_pool(),
        &owner,
        &workspace,
        &chat,
        &id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Attach a computer as a secondary the agent may target when asked.
pub(super) async fn attach_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    device: &str,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    usable_chat(&mut tx, owner, workspace, chat, device).await?;
    sqlx::query(
        "INSERT INTO api_bridge_attachments(owner_id,workspace_id,chat_id,enrollment_id,explicit)
         VALUES($1,$2,$3,$4,true) ON CONFLICT(owner_id,workspace_id,chat_id,enrollment_id)
         DO UPDATE SET explicit=true WHERE NOT api_bridge_attachments.explicit",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(device)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn claim_primary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<PrimaryClaim>,
) -> Result<Json<Value>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let pool = runtime(&state)?.shared_pool();
    let key = host_key(&pool, &headers, "/v1/cloud/bridge/primary").await?;
    let device: String = sqlx::query_scalar(
        "SELECT enrollment_id FROM api_bridge_computers WHERE owner_id=$1 AND key_thumbprint=$2",
    )
    .bind(&owner)
    .bind(key)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::forbidden("computer_not_enrolled"))?;
    primary_for(&pool, &owner, &input.workspace_id, &input.chat_id, &device).await?;
    Ok(Json(json!({"deviceId":device})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PrimaryClaim {
    workspace_id: String,
    chat_id: String,
}

/// Make the computer the owner is sending from the chat's primary. The
/// caller has already proven it holds that computer's enrolled key. The
/// previous primary keeps its running work but is no longer a target unless
/// it was attached explicitly.
pub(super) async fn primary_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    device: &str,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    usable_chat(&mut tx, owner, workspace, chat, device).await?;
    sqlx::query(
        "INSERT INTO api_bridge_attachments(owner_id,workspace_id,chat_id,enrollment_id,explicit)
         VALUES($1,$2,$3,$4,false) ON CONFLICT DO NOTHING",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(device)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO api_bridge_primaries(owner_id,workspace_id,chat_id,enrollment_id) VALUES($1,$2,$3,$4)
         ON CONFLICT(owner_id,workspace_id,chat_id) DO UPDATE SET enrollment_id=excluded.enrollment_id, updated_at=now()",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(device)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn primary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let device: Option<String> = sqlx::query_scalar(
        "SELECT enrollment_id FROM api_bridge_primaries WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_optional(&runtime(&state)?.shared_pool())
    .await?;
    Ok(Json(json!({"deviceId":device})))
}

/// The owner sent from a computer that isn't enabled (or from elsewhere):
/// the chat has no primary until they send from one that is.
pub(super) async fn clear_primary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    sqlx::query(
        "DELETE FROM api_bridge_primaries WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .execute(&runtime(&state)?.shared_pool())
    .await?;
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
