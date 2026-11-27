//! A host advertises one immutable command per task ID. Chat callers supply only that ID,
//! or, for a computer's shell task, the command an owner approves before it runs.
use super::{bridge_approvals::personal_owner, bridge_proof::host_key, *};
use zuse_protocol::computer_tasks::{self as protocol, UNKNOWN_AFTER_SECS};

/// Undecided requests a chat may hold at once; more wait for the owner first.
const MAX_PENDING_PER_CHAT: i64 = 10;
/// Bytes of intermediate or final output a host may publish per request.
const MAX_OUTPUT_BYTES: usize = 98_304;

/// A request's grant still holds: decided once, or covered by a remaining
/// chat or computer grant. Shared by claim and every running check.
const GRANTED: &str = "(r.decision='once'
    OR (r.decision='session' AND EXISTS(SELECT FROM api_bridge_session_grants g
        WHERE (g.owner_id,g.workspace_id,g.chat_id,g.enrollment_id)=(r.owner_id,r.workspace_id,r.chat_id,r.enrollment_id)))
    OR (r.decision='always' AND EXISTS(SELECT FROM api_bridge_always_grants g
        WHERE g.owner_id=r.owner_id AND g.enrollment_id=r.enrollment_id)))";

/// Whether computer `c` is the one the owner is typing on in chat ($2, $3):
/// `primary`, or another of their computers: `secondary`. Agent-facing only.
const ROLE: &str = "CASE WHEN EXISTS(SELECT FROM api_bridge_primaries p
    WHERE p.owner_id=c.owner_id AND p.workspace_id=$2 AND p.chat_id=$3 AND p.enrollment_id=c.enrollment_id)
    THEN 'primary' ELSE 'secondary' END";

/// The owner's active chat on a usable cloud computer.
const OWNED_CHAT: &str = "EXISTS(SELECT FROM api_cloud_workspaces w JOIN zuse_rust_task_command_sessions s USING(workspace_id)
    WHERE w.account_id=$1 AND w.workspace_id=$2 AND s.session_id=$3 AND s.state='active'
      AND w.desired_state IN ('ready','paused') AND w.state NOT IN ('archived','archiving','deleting','deleted'))";

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Action {
    Offer {
        task: uuid::Uuid,
        #[serde(default)]
        dynamic: bool,
        command: String,
        cwd: String,
        user: String,
    },
    Claim {
        task: uuid::Uuid,
    },
    Check {
        task: uuid::Uuid,
        request: uuid::Uuid,
        #[serde(default)]
        output: Option<String>,
    },
    Finish {
        task: uuid::Uuid,
        request: uuid::Uuid,
        output: String,
        status: String,
    },
}

pub(super) async fn host(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(action): Json<Action>,
) -> Result<Json<Value>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    let pool = runtime(&state)?.shared_pool();
    let key = host_key(&pool, &headers, "/v1/cloud/bridge/host-task").await?;
    let device: String = sqlx::query_scalar(
        "SELECT enrollment_id FROM api_bridge_computers WHERE owner_id=$1 AND key_thumbprint=$2",
    )
    .bind(&owner)
    .bind(key)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::forbidden("computer_not_enrolled"))?;
    Ok(Json(host_action(&pool, &owner, &device, action).await?))
}

/// One host action for an authenticated enrollment.
pub(super) async fn host_action(
    pool: &sqlx::PgPool,
    owner: &str,
    device: &str,
    action: Action,
) -> Result<Value, ApiError> {
    Ok(match action {
        Action::Offer {
            task,
            dynamic,
            command,
            cwd,
            user,
        } => {
            if [(&command, 16384), (&cwd, 4096), (&user, 256)]
                .iter()
                .any(|(s, n)| s.is_empty() || s.len() > *n)
            {
                return Err(ApiError::bad_request("invalid_task"));
            }
            let changed = sqlx::query(
                "INSERT INTO api_bridge_tasks(task_id,owner_id,enrollment_id,command,cwd,os_user,dynamic,expires_at)
                 VALUES($1,$2,$3,$4,$5,$6,$7,now()+interval '20 seconds')
                 ON CONFLICT(task_id) DO UPDATE SET expires_at=excluded.expires_at
                 WHERE (api_bridge_tasks.owner_id,api_bridge_tasks.enrollment_id,api_bridge_tasks.dynamic,
                        api_bridge_tasks.command,api_bridge_tasks.cwd,api_bridge_tasks.os_user)
                     =(excluded.owner_id,excluded.enrollment_id,excluded.dynamic,excluded.command,excluded.cwd,excluded.os_user)",
            )
            .bind(task.to_string())
            .bind(owner)
            .bind(device)
            .bind(command)
            .bind(cwd)
            .bind(user)
            .bind(dynamic)
            .execute(pool)
            .await?;
            if changed.rows_affected() != 1 {
                return Err(ApiError::conflict("task_id_in_use"));
            }
            sqlx::query(
                "DELETE FROM api_bridge_tasks t WHERE t.owner_id=$1 AND t.enrollment_id=$2
                   AND t.expires_at < now()-interval '1 day'
                   AND NOT EXISTS (SELECT FROM api_bridge_runs b WHERE b.task_id=t.task_id AND b.result IS NULL)",
            )
            .bind(owner)
            .bind(device)
            .execute(pool)
            .await?;
            Value::Null
        }
        Action::Claim { task } => {
            let claimed: Option<Value> = sqlx::query_scalar(&format!(
                "WITH next AS (
                    SELECT b.request_id FROM api_bridge_requests r JOIN api_bridge_runs b USING(request_id)
                    JOIN api_bridge_tasks t ON t.task_id=b.task_id
                    JOIN api_bridge_tasks cur ON cur.task_id=$3
                    WHERE r.owner_id=$1 AND r.enrollment_id=$2 AND NOT b.claimed AND b.result IS NULL
                      AND cur.owner_id=$1 AND cur.enrollment_id=$2 AND cur.expires_at>now()
                      -- The same offer from an earlier connector session (a reconnect or
                      -- restart gets a new task ID) still reaches its approved requests.
                      AND (b.task_id=$3 OR (t.owner_id,t.enrollment_id,t.dynamic,t.command,t.cwd,t.os_user)
                          =(cur.owner_id,cur.enrollment_id,cur.dynamic,cur.command,cur.cwd,cur.os_user))
                      AND {GRANTED}
                    ORDER BY b.created_at,r.request_id LIMIT 1 FOR UPDATE OF b SKIP LOCKED)
                 UPDATE api_bridge_runs b SET claimed=true,claimed_at=now(),heartbeat_at=now(),task_id=$3
                 FROM next, api_bridge_requests r
                 WHERE b.request_id=next.request_id AND r.request_id=b.request_id AND NOT b.claimed
                 RETURNING jsonb_build_object('id',r.request_id,'command',r.command,'cwd',r.cwd,'timeoutMs',r.timeout_ms)"
            ))
            .bind(owner)
            .bind(device)
            .bind(task.to_string())
            .fetch_optional(pool)
            .await?;
            claimed.unwrap_or(Value::Null)
        }
        Action::Check {
            task,
            request,
            output,
        } => {
            if output.as_ref().is_some_and(|o| o.len() > MAX_OUTPUT_BYTES) {
                return Err(ApiError::bad_request("output_too_large"));
            }
            // A run past its own deadline (plus the unknown window) is no
            // longer authorized; the host stops it.
            let allowed: bool = sqlx::query_scalar(&format!(
                "WITH live AS (
                    UPDATE api_bridge_runs b SET heartbeat_at=now(), progress=coalesce($5,b.progress)
                    FROM api_bridge_requests r
                    WHERE b.request_id=$1::text::uuid AND r.request_id=b.request_id AND b.task_id=$2
                      AND r.owner_id=$3 AND r.enrollment_id=$4 AND b.claimed AND b.result IS NULL
                      AND b.claimed_at + make_interval(secs => r.timeout_ms / 1000.0 + {UNKNOWN_AFTER_SECS}) > now()
                      AND {GRANTED}
                    RETURNING b.task_id)
                 SELECT EXISTS(SELECT FROM live)"
            ))
            .bind(request.to_string())
            .bind(task.to_string())
            .bind(owner)
            .bind(device)
            .bind(output)
            .fetch_one(pool)
            .await?;
            if allowed {
                sqlx::query(
                    "UPDATE api_bridge_tasks SET expires_at=now()+interval '20 seconds'
                     WHERE task_id=$1 AND owner_id=$2 AND enrollment_id=$3",
                )
                .bind(task.to_string())
                .bind(owner)
                .bind(device)
                .execute(pool)
                .await?;
            }
            json!(allowed)
        }
        Action::Finish {
            task,
            request,
            output,
            status,
        } => {
            if output.len() > MAX_OUTPUT_BYTES || status.is_empty() || status.len() > 128 {
                return Err(ApiError::bad_request("invalid_result"));
            }
            let result = json!({"output":output,"status":status});
            let changed = sqlx::query(
                "UPDATE api_bridge_runs b SET result=$1, heartbeat_at=now() FROM api_bridge_requests r
                 WHERE r.request_id=b.request_id AND r.owner_id=$2 AND r.enrollment_id=$3 AND b.task_id=$4
                   AND b.request_id=$5::text::uuid AND b.claimed AND (b.result IS NULL OR b.result=$1)",
            )
            .bind(&result)
            .bind(owner)
            .bind(device)
            .bind(task.to_string())
            .bind(request.to_string())
            .execute(pool)
            .await?;
            if changed.rows_affected() != 1 {
                // The owner stopped it first; the stop stays the outcome.
                let cancelled: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT FROM api_bridge_runs b JOIN api_bridge_requests r USING(request_id)
                     WHERE b.request_id=$1::text::uuid AND r.owner_id=$2 AND r.enrollment_id=$3
                       AND b.task_id=$4 AND b.result->>'status'='Cancelled')",
                )
                .bind(request.to_string())
                .bind(owner)
                .bind(device)
                .bind(task.to_string())
                .fetch_one(pool)
                .await?;
                if !cancelled {
                    return Err(ApiError::conflict("result_unavailable"));
                }
            }
            Value::Null
        }
    })
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    Ok(Json(
        list_for(&runtime(&state)?.shared_pool(), &owner, &workspace, &chat).await?,
    ))
}

pub(super) async fn list_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT jsonb_build_object('task',t.task_id,'deviceId',t.enrollment_id,'dynamic',t.dynamic,
            'command',t.command,'cwd',t.cwd,'user',t.os_user,'computer',c.label,'role',{ROLE})
         FROM api_bridge_tasks t JOIN api_bridge_computers c USING(owner_id,enrollment_id)
         WHERE c.owner_id=$1 AND t.expires_at>now() AND {OWNED_CHAT}
         ORDER BY c.label,t.command"
    ))
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_all(pool)
    .await?)
}

/// The owner's enrolled computers, the one they are typing on first, online
/// or not, with their shell connector's details while online.
pub(super) async fn computers_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT jsonb_build_object('deviceId',c.enrollment_id,'name',c.label,'platform',c.platform,
            'role',{ROLE},'online',t.task_id IS NOT NULL,'shell',t.command,'user',t.os_user,
            'cwd',CASE WHEN t.task_id IS NOT NULL THEN coalesce(a.synced_cwd,t.cwd) END,
            'syncedCheckout',a.synced_cwd,
            'defaultTimeoutSeconds',$4::int,'maxTimeoutSeconds',$5::int)
         FROM api_bridge_computers c
         LEFT JOIN LATERAL (SELECT task_id,command,os_user,cwd FROM api_bridge_tasks t
             WHERE t.owner_id=c.owner_id AND t.enrollment_id=c.enrollment_id AND t.dynamic AND t.expires_at>now()
             ORDER BY t.expires_at DESC LIMIT 1) t ON true
         LEFT JOIN api_bridge_attachments a ON a.owner_id=c.owner_id AND a.workspace_id=$2
             AND a.chat_id=$3 AND a.enrollment_id=c.enrollment_id
         WHERE c.owner_id=$1 AND {OWNED_CHAT}
         ORDER BY {ROLE} <> 'primary', lower(c.label), c.enrollment_id"
    ))
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(protocol::DEFAULT_TIMEOUT_SECS as i32)
    .bind(protocol::MAX_TIMEOUT_SECS as i32)
    .fetch_all(pool)
    .await?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    request_id: uuid::Uuid,
    task: uuid::Uuid,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u32>,
}

pub(super) async fn request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
    Json(input): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    Ok(Json(
        request_for(
            &runtime(&state)?.shared_pool(),
            &owner,
            &workspace,
            &chat,
            input,
        )
        .await?,
    ))
}

pub(super) async fn request_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    input: Request,
) -> Result<Value, ApiError> {
    if input
        .command
        .as_ref()
        .is_some_and(|c| c.trim().is_empty() || c.len() > 16384)
        || input
            .cwd
            .as_ref()
            .is_some_and(|c| c.is_empty() || c.len() > 4096)
    {
        return Err(ApiError::bad_request("invalid_computer_command"));
    }
    let timeout_ms = protocol::timeout_ms(input.timeout_seconds)
        .ok_or_else(|| ApiError::bad_request("invalid_computer_timeout"))?;
    let id = input.request_id.to_string();
    let mut tx = pool.begin().await?;
    // Serialize retries of the same logical request across API processes.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    if let Some(existing) = existing_request(&mut tx, &id, owner, workspace, chat).await? {
        return Ok(existing);
    }
    // Serialize new requests per chat so the pending cap holds under races.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,1))")
        .bind(format!("{owner}\n{workspace}\n{chat}"))
        .execute(&mut *tx)
        .await?;
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_bridge_requests r JOIN api_bridge_runs b USING(request_id)
         WHERE r.owner_id=$1 AND r.workspace_id=$2 AND r.chat_id=$3 AND r.decision IS NULL
           AND NOT b.claimed AND b.result IS NULL",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_one(&mut *tx)
    .await?;
    if waiting >= MAX_PENDING_PER_CHAT {
        return Err(ApiError::conflict("too_many_pending_computer_requests"));
    }
    // Lock the owner's chat and computer row so an archive commits either
    // before this request (and it is refused) or after it (and its cleanup
    // removes the request).
    sqlx::query(
        "SELECT 1 FROM api_cloud_workspaces w JOIN zuse_rust_task_command_sessions s USING(workspace_id)
         WHERE w.account_id=$1 AND w.workspace_id=$2 AND s.session_id=$3 AND s.state='active'
           AND w.desired_state IN ('ready','paused') AND w.state NOT IN ('archived','archiving','deleting','deleted')
         FOR SHARE OF w, s",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("bridge_chat_unavailable"))?;
    // Serialize with revoking this computer's "always" grant, so a request
    // can't keep an "always" decision its grant no longer backs.
    let device: Option<String> = sqlx::query_scalar(
        "SELECT enrollment_id FROM api_bridge_tasks WHERE task_id=$1 AND owner_id=$2",
    )
    .bind(input.task.to_string())
    .bind(owner)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(device) = &device {
        grant_lock(&mut tx, owner, device).await?;
    }
    // Any of the owner's computers can serve the chat without attaching it.
    // The row holds the chat's requests and grants for that computer.
    sqlx::query(
        "INSERT INTO api_bridge_attachments(owner_id,workspace_id,chat_id,enrollment_id,explicit)
         SELECT owner_id,$2,$3,enrollment_id,false FROM api_bridge_tasks
         WHERE task_id=$4 AND owner_id=$1 AND expires_at>now() ON CONFLICT DO NOTHING",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(input.task.to_string())
    .execute(&mut *tx)
    .await?;
    let added = sqlx::query(&format!(
        "INSERT INTO api_bridge_requests(request_id,owner_id,workspace_id,chat_id,enrollment_id,computer_name,
            os_user,shell,cwd,command,timeout_ms,decision,role)
         SELECT $4::text::uuid,owner_id,workspace_id,chat_id,enrollment_id,label,os_user,
            CASE WHEN dynamic THEN t.command ELSE 'direct executable' END,
            coalesce($7,CASE WHEN dynamic THEN a.synced_cwd END,t.cwd),coalesce($6,t.command),$8,
            CASE WHEN EXISTS(SELECT FROM api_bridge_always_grants g WHERE g.owner_id=a.owner_id AND g.enrollment_id=a.enrollment_id) THEN 'always'
                 WHEN EXISTS(SELECT FROM api_bridge_session_grants g
                     WHERE (g.owner_id,g.workspace_id,g.chat_id,g.enrollment_id)=(a.owner_id,a.workspace_id,a.chat_id,a.enrollment_id)) THEN 'session' END,
            {ROLE}
         FROM api_bridge_tasks t JOIN api_bridge_computers c USING(owner_id,enrollment_id)
         JOIN api_bridge_attachments a USING(owner_id,enrollment_id)
         WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND task_id=$5 AND expires_at>now()
           AND (dynamic OR ($6::text IS NULL AND $7::text IS NULL))"
    ))
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(&id)
    .bind(input.task.to_string())
    .bind(&input.command)
    .bind(&input.cwd)
    .bind(timeout_ms)
    .execute(&mut *tx)
    .await?;
    if added.rows_affected() != 1 {
        return Err(ApiError::conflict("task_offline_or_detached"));
    }
    sqlx::query(
        "INSERT INTO api_bridge_request_ids(request_id,owner_id,workspace_id,chat_id) VALUES($1::text::uuid,$2,$3,$4)",
    )
    .bind(&id)
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO api_bridge_runs(request_id,task_id) VALUES($1::text::uuid,$2)")
        .bind(&id)
        .bind(input.task.to_string())
        .execute(&mut *tx)
        .await?;
    let created: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('id',request_id,'deviceId',enrollment_id,'computer',computer_name,'role',role,'cwd',cwd)
         FROM api_bridge_requests WHERE request_id=$1::text::uuid",
    )
    .bind(&id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(created)
}

/// A request ID seen before: the same request again (its summary), a settled
/// one whose rows were removed (refused), or another chat's (refused).
async fn existing_request(
    tx: &mut sqlx::PgConnection,
    id: &str,
    owner: &str,
    workspace: &str,
    chat: &str,
) -> Result<Option<Value>, ApiError> {
    let scope: Option<bool> = sqlx::query_scalar(
        "SELECT owner_id=$2 AND workspace_id=$3 AND chat_id=$4 FROM api_bridge_request_ids
         WHERE request_id=$1::text::uuid",
    )
    .bind(id)
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_optional(&mut *tx)
    .await?;
    match scope {
        None => Ok(None),
        Some(false) => Err(ApiError::conflict("computer_request_id_reused")),
        Some(true) => {
            let summary: Option<Value> = sqlx::query_scalar(
                "SELECT jsonb_build_object('id',request_id,'deviceId',enrollment_id,'computer',computer_name,'role',role,'cwd',cwd)
                 FROM api_bridge_requests WHERE request_id=$1::text::uuid",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
            summary
                .map(Some)
                .ok_or_else(|| ApiError::conflict("computer_request_settled"))
        }
    }
}

/// Held while a computer's "always" grant is read into a new request or
/// revoked.
pub(super) async fn grant_lock(
    tx: &mut sqlx::PgConnection,
    owner: &str,
    device: &str,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,2))")
        .bind(format!("{owner}\n{device}"))
        .execute(&mut *tx)
        .await?;
    Ok(())
}

pub(super) async fn results(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    Ok(Json(
        results_for(&runtime(&state)?.shared_pool(), &owner, &workspace, &chat).await?,
    ))
}

/// Recent requests for a chat, newest first. `state` is one of `pending`,
/// `approved`, `running`, `unknown`, `finished`, `cancelled` or `denied`;
/// `result.status` is the readable form. A claimed request with no report
/// inside the unknown window, or past its own deadline, is `unknown` and is
/// never re-run.
pub(super) async fn results_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
) -> Result<Vec<Value>, ApiError> {
    Ok(sqlx::query_scalar(&format!(
        "WITH rows AS (
            SELECT r.*, b.claimed, b.claimed_at, b.result, b.progress,
              CASE
                WHEN b.result->>'status'='Cancelled' THEN 'cancelled'
                WHEN b.result IS NOT NULL THEN 'finished'
                WHEN r.decision='deny' THEN 'denied'
                WHEN NOT b.claimed AND r.decision IS NULL THEN 'pending'
                WHEN NOT b.claimed THEN 'approved'
                WHEN coalesce(b.heartbeat_at,b.claimed_at) < now()-interval '{UNKNOWN_AFTER_SECS} seconds'
                  OR b.claimed_at + make_interval(secs => r.timeout_ms / 1000.0 + {UNKNOWN_AFTER_SECS}) < now()
                  THEN 'unknown'
                ELSE 'running' END AS run_state
            FROM api_bridge_runs b JOIN api_bridge_requests r USING(request_id)
            WHERE r.owner_id=$1 AND r.workspace_id=$2 AND r.chat_id=$3
            ORDER BY r.created_at DESC, r.request_id LIMIT 30)
         SELECT jsonb_build_object('id',request_id,'deviceId',enrollment_id,'computer',computer_name,
            'role',role,'command',command,'cwd',cwd,'timeoutMs',timeout_ms,
            'createdAt',(extract(epoch FROM created_at)*1000)::bigint,'state',run_state,
            'result',CASE WHEN result IS NOT NULL THEN result ELSE jsonb_build_object(
                'status',CASE run_state WHEN 'denied' THEN 'Denied' WHEN 'pending' THEN 'Waiting for approval'
                    WHEN 'approved' THEN 'Approved' WHEN 'unknown' THEN 'Outcome unknown' ELSE 'Running' END,
                'output',coalesce(progress,'')) END)
         FROM rows ORDER BY created_at DESC, request_id"
    ))
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .fetch_all(pool)
    .await?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RuntimeRequest {
    chat_id: String,
    request: protocol::Request,
}

pub(super) async fn runtime_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workspace): Path<String>,
    Json(input): Json<RuntimeRequest>,
) -> Result<Json<Value>, ApiError> {
    let control = runtime(&state)?;
    let owner = control
        .computer_task_owner(&workspace, &input.chat_id, runtime_bearer(&headers)?)
        .await?;
    let pool = control.shared_pool();
    Ok(Json(
        runtime_request(&pool, &owner, &workspace, &input.chat_id, input.request).await?,
    ))
}

/// A request from the chat's own runtime, after its credential and chat are verified.
pub(super) async fn runtime_request(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    request: protocol::Request,
) -> Result<Value, ApiError> {
    use protocol::Request as Task;
    // A retried submission is recognized before the computer's current
    // offer is resolved: the computer may have gone offline or restarted.
    if let Task::Run { request_id, .. } | Task::Command { request_id, .. } = &request {
        let mut tx = pool.begin().await?;
        let existing =
            existing_request(&mut tx, &request_id.to_string(), owner, workspace, chat).await?;
        tx.commit().await?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
    }
    Ok(match request {
        Task::List => json!(list_for(pool, owner, workspace, chat).await?),
        Task::Computers => json!(computers_for(pool, owner, workspace, chat).await?),
        Task::Run {
            task,
            request_id,
            timeout_seconds,
        } => {
            request_for(
                pool,
                owner,
                workspace,
                chat,
                Request {
                    task,
                    request_id,
                    command: None,
                    cwd: None,
                    timeout_seconds,
                },
            )
            .await?
        }
        Task::Command {
            device_id,
            command,
            cwd,
            request_id,
            timeout_seconds,
        } => {
            // No device means the computer the owner is typing on. Their
            // other computers are only ever explicit targets.
            let device_id = match device_id {
                Some(device) => device,
                None => sqlx::query_scalar(
                    "SELECT enrollment_id FROM api_bridge_primaries
                     WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND enrollment_id IS NOT NULL",
                )
                .bind(owner)
                .bind(workspace)
                .bind(chat)
                .fetch_optional(pool)
                .await?
                .ok_or_else(|| ApiError::conflict("no_primary_computer"))?,
            };
            let enrolled: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT FROM api_bridge_computers WHERE owner_id=$1 AND enrollment_id=$2)",
            )
            .bind(owner)
            .bind(&device_id)
            .fetch_one(pool)
            .await?;
            if !enrolled {
                return Err(ApiError::conflict("computer_not_found"));
            }
            let task: String = sqlx::query_scalar(
                "SELECT task_id FROM api_bridge_tasks WHERE owner_id=$1 AND enrollment_id=$2
                 AND dynamic AND expires_at>now() ORDER BY expires_at DESC LIMIT 1",
            )
            .bind(owner)
            .bind(&device_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| ApiError::conflict("computer_offline"))?;
            request_for(
                pool,
                owner,
                workspace,
                chat,
                Request {
                    task: uuid::Uuid::parse_str(&task)
                        .map_err(|_| ApiError::conflict("invalid_task"))?,
                    request_id,
                    command: Some(command),
                    cwd,
                    timeout_seconds,
                },
            )
            .await?
        }
        Task::Results => json!(results_for(pool, owner, workspace, chat).await?),
    })
}

pub(super) async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, request)): Path<(String, String, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    cancel_for(
        &runtime(&state)?.shared_pool(),
        &owner,
        &workspace,
        &chat,
        request,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stop a request: an unclaimed one is never claimed, and a running one's
/// next check tells its host to stop the process group.
pub(super) async fn cancel_for(
    pool: &sqlx::PgPool,
    owner: &str,
    workspace: &str,
    chat: &str,
    request: uuid::Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE api_bridge_runs b SET claimed=true, result=jsonb_build_object('status','Cancelled','output',coalesce(b.progress,''))
         FROM api_bridge_requests r WHERE r.request_id=b.request_id AND r.owner_id=$1 AND r.workspace_id=$2
           AND r.chat_id=$3 AND r.request_id=$4::text::uuid AND b.result IS NULL",
    )
    .bind(owner)
    .bind(workspace)
    .bind(chat)
    .bind(request.to_string())
    .execute(pool)
    .await?;
    Ok(())
}
