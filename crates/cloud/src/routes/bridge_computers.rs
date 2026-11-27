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
        DO UPDATE SET platform=excluded.platform RETURNING enrollment_id",
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
    let primary = claim_for(
        &pool,
        &owner,
        &input.workspace_id,
        &input.chat_id,
        &device,
        input.command_id.as_deref(),
        input.synced_cwd.as_deref(),
    )
    .await?;
    Ok(Json(json!({"deviceId":primary})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PrimaryClaim {
    workspace_id: String,
    chat_id: String,
    /// The admitted message this claim follows; omitted for a new chat.
    #[serde(default)]
    command_id: Option<String>,
    /// The folder this chat is synced into on the claiming computer; omitted
    /// when it isn't synced there.
    #[serde(default)]
    synced_cwd: Option<String>,
}

/// When the owner's message `command` was admitted in the chat (ms).
async fn admitted_at(
    tx: &mut sqlx::PgConnection,
    workspace: &str,
    chat: &str,
    command: &str,
) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        "SELECT created_at FROM zuse_rust_task_command_actions
         WHERE workspace_id=$1 AND session_id=$2 AND command_id=$3",
    )
    .bind(workspace)
    .bind(chat)
    .bind(command)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("bridge_message_unavailable"))
}

/// Make the computer the owner sent `command` from the chat's default. The
/// caller has already proven it holds that computer's enrolled key. Claims
/// are ordered by their message's admission, so a delayed claim for an
/// earlier message never replaces a later one; a claim without a message
/// (a new chat) only sets a default when there is none. The previous
/// computer keeps its running work. Returns the chat's default after.
#[cfg(test)]
pub(super) async fn primary_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    device: &str,
    command: Option<&str>,
) -> Result<Option<String>, ApiError> {
    claim_for(pool, owner, workspace, chat, device, command, None).await
}

/// A folder path on the owner's computer: absolute (Unix or Windows), at
/// most 4 KiB, without control characters.
fn valid_folder(path: &str) -> bool {
    let bytes = path.as_bytes();
    let absolute = path.starts_with('/')
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'));
    absolute && path.len() <= 4096 && !path.chars().any(char::is_control)
}

/// [`primary_for`], also recording the folder the chat is synced into on
/// that computer (`None`: not synced there). The folder is the default
/// working directory of the chat's commands on that computer and is set
/// whether or not this claim moves the default computer.
pub(super) async fn claim_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    device: &str,
    command: Option<&str>,
    synced: Option<&str>,
) -> Result<Option<String>, ApiError> {
    if synced.is_some_and(|path| !valid_folder(path)) {
        return Err(ApiError::bad_request("invalid_synced_folder"));
    }
    let mut tx = pool.begin().await?;
    usable_chat(&mut tx, owner, workspace, chat, device).await?;
    let ordered_at = match command {
        Some(command) => admitted_at(&mut tx, workspace, chat, command).await?,
        None => 0,
    };
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
        "UPDATE api_bridge_attachments SET synced_cwd=$5
         WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND enrollment_id=$4
           AND synced_cwd IS DISTINCT FROM $5",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(device)
    .bind(synced)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO api_bridge_primaries(owner_id,workspace_id,chat_id,enrollment_id,ordered_at)
         VALUES($1,$2,$3,$4,$5)
         ON CONFLICT(owner_id,workspace_id,chat_id) DO UPDATE
         SET enrollment_id=excluded.enrollment_id, ordered_at=excluded.ordered_at, updated_at=now()
         WHERE api_bridge_primaries.ordered_at < excluded.ordered_at",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(device)
    .bind(ordered_at)
    .execute(&mut *tx)
    .await?;
    let primary: Option<String> = sqlx::query_scalar(
        "SELECT enrollment_id FROM api_bridge_primaries
         WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND enrollment_id IS NOT NULL",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(primary)
}

/// The owner sent `command` from a computer that isn't enabled (or from
/// elsewhere): clear the default unless a later message already set one.
pub(super) async fn clear_primary_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    command: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let before = match command {
        Some(command) => admitted_at(&mut tx, workspace, chat, command).await?,
        None => i64::MAX,
    };
    if before == i64::MAX {
        // Unordered (a new chat): nothing later can exist yet.
        sqlx::query(
            "DELETE FROM api_bridge_primaries WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3",
        )
        .bind(owner)
        .bind(workspace)
        .bind(chat)
        .execute(&mut *tx)
        .await?;
    } else {
        // Keep the row, without a computer, so a delayed claim for an
        // earlier message still loses to this clear.
        sqlx::query(
            "INSERT INTO api_bridge_primaries(owner_id,workspace_id,chat_id,enrollment_id,ordered_at)
             SELECT $1,$2,$3,NULL,$4 WHERE EXISTS(SELECT FROM api_cloud_workspaces
                 WHERE workspace_id=$2 AND account_id=$1)
             ON CONFLICT(owner_id,workspace_id,chat_id) DO UPDATE
             SET enrollment_id=NULL, ordered_at=excluded.ordered_at, updated_at=now()
             WHERE api_bridge_primaries.ordered_at < excluded.ordered_at",
        )
        .bind(owner)
        .bind(workspace)
        .bind(chat)
        .bind(before)
        .execute(&mut *tx)
        .await?;
    }
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
        "SELECT enrollment_id FROM api_bridge_primaries
         WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND enrollment_id IS NOT NULL",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_optional(&runtime(&state)?.shared_pool())
    .await?;
    Ok(Json(json!({"deviceId":device})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClearQuery {
    #[serde(default)]
    command_id: Option<String>,
}

/// The owner sent from a computer that isn't enabled (or from elsewhere):
/// the chat has no default until they send from one that is.
pub(super) async fn clear_primary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
    Query(query): Query<ClearQuery>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    clear_primary_for(
        &runtime(&state)?.shared_pool(),
        &owner,
        &workspace,
        &chat,
        query.command_id.as_deref(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rename {
    name: String,
}

/// The owner renames one of their computers; approvals and the agent use it.
pub(super) async fn rename(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Rename>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    rename_for(&runtime(&state)?.shared_pool(), &owner, &id, &input.name).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn rename_for(
    pool: &sqlx::PgPool,
    owner: &str,
    device: &str,
    name: &str,
) -> Result<(), ApiError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("invalid_bridge_computer"));
    }
    let renamed = sqlx::query(
        "UPDATE api_bridge_computers SET label=$3 WHERE owner_id=$1 AND enrollment_id=$2",
    )
    .bind(owner)
    .bind(device)
    .bind(name)
    .execute(pool)
    .await?;
    if renamed.rows_affected() != 1 {
        return Err(ApiError::not_found("bridge_computer_unavailable"));
    }
    Ok(())
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
