//! Owner-only decisions about immutable device command previews.
use super::*;

#[derive(Clone, Copy, Debug, Deserialize)]
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

pub(super) async fn personal_owner(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, ApiError> {
    let access = state.access(headers, AccessKind::Content).await?;
    if !matches!(access.scope, access::Scope::Personal) {
        return Err(ApiError::forbidden("bridge_personal_account_required"));
    }
    Ok(access.actor.account_id)
}

/// The owner's computer exists and is in a usable state.
const OPEN_WORKSPACE: &str = "EXISTS (SELECT FROM api_cloud_workspaces w WHERE w.workspace_id=$2 AND w.account_id=$1
    AND w.desired_state IN ('ready','paused') AND w.state NOT IN ('archived','archiving','deleting','deleted'))";

pub(super) async fn pending(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    Ok(Json(
        pending_for(&runtime(&state)?.shared_pool(), &owner, &workspace, &chat).await?,
    ))
}

/// Requests waiting for the owner's decision, oldest first.
pub(super) async fn pending_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT jsonb_build_object('id',r.request_id,'deviceId',r.enrollment_id,
            'computerName',r.computer_name,'osUser',r.os_user,'shell',r.shell,'cwd',r.cwd,
            'command',r.command,'timeoutMs',r.timeout_ms,'role',r.role,
            'createdAt',(extract(epoch FROM r.created_at)*1000)::bigint)
         FROM api_bridge_requests r JOIN api_bridge_runs b USING(request_id)
         WHERE r.owner_id=$1 AND r.workspace_id=$2 AND r.chat_id=$3 AND r.decision IS NULL
           AND NOT b.claimed AND b.result IS NULL AND {OPEN_WORKSPACE}
         ORDER BY r.created_at, r.request_id"
    ))
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_all(pool)
    .await?)
}

pub(super) async fn decide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, request)): Path<(String, String, uuid::Uuid)>,
    Json(choice): Json<Choice>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    decide_for(
        &runtime(&state)?.shared_pool(),
        &owner,
        &workspace,
        &chat,
        request,
        choice.decision,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Record the owner's decision once. "This session" and "always" also grant
/// later requests from this chat, or from any of the owner's chats, on the
/// same enrolled computer.
pub(super) async fn decide_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    request: uuid::Uuid,
    decision: Decision,
) -> Result<(), ApiError> {
    let text = match decision {
        Decision::Once => "once",
        Decision::Session => "session",
        Decision::Always => "always",
        Decision::Deny => "deny",
    };
    let mut tx = pool.begin().await?;
    let device: String = sqlx::query_scalar(&format!(
        "UPDATE api_bridge_requests r SET decision=$5
         WHERE r.owner_id=$1 AND r.workspace_id=$2 AND r.chat_id=$3 AND r.request_id=$4::text::uuid
           AND {OPEN_WORKSPACE} AND r.decision IS NULL
           AND EXISTS (SELECT FROM api_bridge_runs b WHERE b.request_id=r.request_id AND NOT b.claimed AND b.result IS NULL)
         RETURNING r.enrollment_id"
    ))
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(request.to_string())
    .bind(text)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::conflict("bridge_request_unavailable"))?;
    match decision {
        Decision::Session => {
            sqlx::query(
                "INSERT INTO api_bridge_session_grants(owner_id,workspace_id,chat_id,enrollment_id)
                 VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
            )
            .bind(owner)
            .bind(workspace)
            .bind(chat)
            .bind(device)
            .execute(&mut *tx)
            .await?;
        }
        Decision::Always => {
            sqlx::query(
                "INSERT INTO api_bridge_always_grants(owner_id,enrollment_id) VALUES ($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(owner)
            .bind(device)
            .execute(&mut *tx)
            .await?;
        }
        Decision::Once | Decision::Deny => {}
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn detach(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, enrollment)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    sqlx::query(
        "DELETE FROM api_bridge_attachments WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND enrollment_id=$4",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(enrollment)
    .execute(&runtime(&state)?.shared_pool())
    .await?;
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
