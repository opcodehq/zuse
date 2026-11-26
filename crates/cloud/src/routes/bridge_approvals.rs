//! Owner-only decisions about immutable device command previews.
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Decision {
    Once,
    Session,
    Always,
    Deny,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Choice {
    decision: Decision,
}

pub(super) async fn personal_owner(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let access = state.access(headers, AccessKind::Content).await?;
    if !matches!(access.scope, access::Scope::Personal) {
        return Err(ApiError::forbidden("bridge_personal_account_required"));
    }
    Ok(access.actor.account_id)
}

pub(super) async fn pending(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let requests = sqlx::query_scalar(
        "SELECT jsonb_build_object('id',request_id,'deviceId',enrollment_id,
         'computerName',computer_name,'osUser',os_user,'shell',shell,'cwd',cwd,
         'command',command,'timeoutMs',timeout_ms) FROM api_bridge_requests
         WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND decision IS NULL
         AND EXISTS (SELECT FROM api_cloud_workspaces w WHERE w.workspace_id=$2 AND w.account_id=$1
                     AND w.desired_state IN ('ready','paused') AND w.state NOT IN ('archived','archiving','deleting','deleted'))
         ORDER BY request_id",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_all(&runtime(&state)?.shared_pool())
    .await?;
    Ok(Json(requests))
}

pub(super) async fn decide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, request)): Path<(String, String, uuid::Uuid)>,
    Json(choice): Json<Choice>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let decision = match choice.decision {
        Decision::Once => "once",
        Decision::Session => "session",
        Decision::Always => "always",
        Decision::Deny => "deny",
    };
    let pool = runtime(&state)?.shared_pool();
    let mut tx = pool.begin().await?;
    let device: String = sqlx::query_scalar(
        "UPDATE api_bridge_requests SET decision=$5
         WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND request_id=$4::text::uuid
         AND EXISTS (SELECT FROM api_cloud_workspaces w WHERE w.workspace_id=$2 AND w.account_id=$1
                     AND w.desired_state IN ('ready','paused') AND w.state NOT IN ('archived','archiving','deleting','deleted'))
         AND decision IS NULL RETURNING enrollment_id",
    )
    .bind(&owner)
    .bind(&workspace)
    .bind(&chat)
    .bind(request.to_string())
    .bind(decision)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::conflict("bridge_request_unavailable"))?;
    match choice.decision {
        Decision::Session => {
            sqlx::query("INSERT INTO api_bridge_session_grants(owner_id,workspace_id,chat_id,enrollment_id) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
                .bind(&owner).bind(&workspace).bind(&chat).bind(device).execute(&mut *tx).await?;
        }
        Decision::Always => {
            sqlx::query("INSERT INTO api_bridge_always_grants(owner_id,enrollment_id) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                .bind(&owner).bind(device).execute(&mut *tx).await?;
        }
        Decision::Once | Decision::Deny => {}
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn detach(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, enrollment)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    sqlx::query("DELETE FROM api_bridge_attachments WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND enrollment_id=$4")
        .bind(owner).bind(workspace).bind(chat).bind(enrollment)
        .execute(&runtime(&state)?.shared_pool()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn revoke_always(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(enrollment): Path<String>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    sqlx::query("DELETE FROM api_bridge_always_grants WHERE owner_id=$1 AND enrollment_id=$2")
        .bind(owner)
        .bind(enrollment)
        .execute(&runtime(&state)?.shared_pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
