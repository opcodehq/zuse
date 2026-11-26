//! A host advertises one immutable command per task ID. Chat callers supply only that ID.
use super::{bridge_approvals::personal_owner, bridge_computers::host_key, *};

#[derive(Deserialize)]
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
    let key = host_key(&state, &headers, "/v1/cloud/bridge/host-task").await?;
    let pool = runtime(&state)?.shared_pool();
    let device: String = sqlx::query_scalar(
        "SELECT enrollment_id FROM api_bridge_computers WHERE owner_id=$1 AND key_thumbprint=$2",
    )
    .bind(&owner)
    .bind(key)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::forbidden("computer_not_enrolled"))?;
    let response = match action {
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
            let changed=sqlx::query("INSERT INTO api_bridge_tasks(task_id,owner_id,enrollment_id,command,cwd,os_user,dynamic,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,now()+interval '20 seconds')
                ON CONFLICT(task_id) DO UPDATE SET expires_at=excluded.expires_at
                WHERE (api_bridge_tasks.owner_id,api_bridge_tasks.enrollment_id,api_bridge_tasks.dynamic,api_bridge_tasks.command,api_bridge_tasks.cwd,api_bridge_tasks.os_user)
                =(excluded.owner_id,excluded.enrollment_id,excluded.dynamic,excluded.command,excluded.cwd,excluded.os_user)")
                .bind(task.to_string()).bind(owner).bind(device).bind(command).bind(cwd).bind(user).bind(dynamic).execute(&pool).await?;
            if changed.rows_affected() != 1 {
                return Err(ApiError::conflict("task_id_in_use"));
            }
            Value::Null
        }
        Action::Claim { task } => {
            let id:Option<String>=sqlx::query_scalar("UPDATE api_bridge_runs SET claimed=true,claimed_at=now() WHERE request_id=(
                SELECT r.request_id FROM api_bridge_requests r JOIN api_bridge_runs b USING(request_id)
                JOIN api_bridge_tasks t ON t.task_id=b.task_id
                WHERE r.owner_id=$1 AND r.enrollment_id=$2 AND b.task_id=$3 AND NOT b.claimed
                AND t.expires_at>now() AND (r.decision='once' OR (r.decision='session' AND EXISTS(
                SELECT FROM api_bridge_session_grants g WHERE (g.owner_id,g.workspace_id,g.chat_id,g.enrollment_id)=(r.owner_id,r.workspace_id,r.chat_id,r.enrollment_id)))
                OR (r.decision='always' AND EXISTS(SELECT FROM api_bridge_always_grants g WHERE g.owner_id=r.owner_id AND g.enrollment_id=r.enrollment_id)))
                ORDER BY b.created_at,r.request_id LIMIT 1 FOR UPDATE OF b SKIP LOCKED) AND NOT claimed RETURNING request_id::text")
                .bind(owner).bind(device).bind(task.to_string()).fetch_optional(&pool).await?;
            if let Some(id) = id {
                let preview:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',request_id,'command',command,'cwd',cwd) FROM api_bridge_requests WHERE request_id=$1::text::uuid").bind(id).fetch_one(&pool).await?;
                preview
            } else {
                Value::Null
            }
        }
        Action::Check {
            task,
            request,
            output,
        } => {
            let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT FROM api_bridge_runs b JOIN api_bridge_requests r USING(request_id)
                WHERE b.request_id=$1::text::uuid AND b.task_id=$2 AND r.owner_id=$3 AND r.enrollment_id=$4 AND b.claimed AND b.result IS NULL
                AND (r.decision='once' OR (r.decision='session' AND EXISTS(SELECT FROM api_bridge_session_grants g
                WHERE (g.owner_id,g.workspace_id,g.chat_id,g.enrollment_id)=(r.owner_id,r.workspace_id,r.chat_id,r.enrollment_id)))
                OR (r.decision='always' AND EXISTS(SELECT FROM api_bridge_always_grants g WHERE g.owner_id=r.owner_id AND g.enrollment_id=r.enrollment_id))))")
                .bind(request.to_string()).bind(task.to_string()).bind(owner).bind(device).fetch_one(&pool).await?;
            if allowed {
                if let Some(output) = output {
                    if output.len() > 98304 {
                        return Err(ApiError::bad_request("output_too_large"));
                    }
                    sqlx::query("UPDATE api_bridge_runs SET progress=$2 WHERE request_id=$1::text::uuid AND result IS NULL").bind(request.to_string()).bind(output).execute(&pool).await?;
                }
                sqlx::query("UPDATE api_bridge_tasks SET expires_at=now()+interval '20 seconds' WHERE task_id=$1").bind(task.to_string()).execute(&pool).await?;
            }
            json!(allowed)
        }
        Action::Finish {
            task,
            request,
            output,
            status,
        } => {
            if output.len() > 98304 || status.len() > 128 {
                return Err(ApiError::bad_request("invalid_result"));
            }
            let changed=sqlx::query("UPDATE api_bridge_runs b SET result=$1 FROM api_bridge_requests r WHERE r.request_id=b.request_id AND r.owner_id=$2 AND r.enrollment_id=$3 AND b.task_id=$4 AND b.request_id=$5::text::uuid AND b.claimed AND (b.result IS NULL OR b.result=$1)")
                .bind(json!({"output":output,"status":status})).bind(owner).bind(device).bind(task.to_string()).bind(request.to_string()).execute(&pool).await?;
            if changed.rows_affected() != 1 {
                return Err(ApiError::conflict("result_unavailable"));
            }
            Value::Null
        }
    };
    Ok(Json(response))
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    list_for(runtime(&state)?.shared_pool(), owner, workspace, chat).await
}
pub(super) async fn list_for(
    pool: sqlx::PgPool,
    owner: String,
    workspace: String,
    chat: String,
) -> Result<Json<Vec<Value>>, ApiError> {
    Ok(Json(sqlx::query_scalar("SELECT jsonb_build_object('task',task_id,'deviceId',enrollment_id,'dynamic',dynamic,'command',command,'cwd',cwd,'user',os_user,'computer',label)
        FROM api_bridge_tasks JOIN api_bridge_computers USING(owner_id,enrollment_id) JOIN api_bridge_attachments USING(owner_id,enrollment_id)
        WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND expires_at>now() ORDER BY label,command")
        .bind(owner).bind(workspace).bind(chat).fetch_all(&pool).await?))
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
}
pub(super) async fn request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
    Json(input): Json<Request>,
) -> Result<Json<Value>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    request_for(
        runtime(&state)?.shared_pool(),
        owner,
        workspace,
        chat,
        input,
    )
    .await
}
pub(super) async fn request_for(
    pool: sqlx::PgPool,
    owner: String,
    workspace: String,
    chat: String,
    input: Request,
) -> Result<Json<Value>, ApiError> {
    if input
        .command
        .as_ref()
        .is_some_and(|c| c.is_empty() || c.len() > 16384)
        || input
            .cwd
            .as_ref()
            .is_some_and(|c| c.is_empty() || c.len() > 4096)
    {
        return Err(ApiError::bad_request("invalid_computer_command"));
    }
    let id = input.request_id.to_string();
    let mut tx = pool.begin().await?;
    // Serialize retries of the same logical request across API processes.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    let previous:Option<bool>=sqlx::query_scalar("SELECT r.owner_id=$2 AND r.workspace_id=$3 AND r.chat_id=$4 AND b.task_id=$5 FROM api_bridge_requests r JOIN api_bridge_runs b USING(request_id) WHERE r.request_id=$1::text::uuid")
        .bind(&id).bind(&owner).bind(&workspace).bind(&chat).bind(input.task.to_string()).fetch_optional(&mut *tx).await?;
    if let Some(same) = previous {
        if !same {
            return Err(ApiError::conflict("computer_request_id_reused"));
        }
        return Ok(Json(json!({"id":id})));
    }
    let added=sqlx::query("INSERT INTO api_bridge_requests(request_id,owner_id,workspace_id,chat_id,enrollment_id,computer_name,os_user,shell,cwd,command,timeout_ms,decision)
        SELECT $4::text::uuid,owner_id,workspace_id,chat_id,enrollment_id,label,os_user,CASE WHEN dynamic THEN t.command ELSE 'direct executable' END,coalesce($7,t.cwd),coalesce($6,t.command),60000,
        CASE WHEN EXISTS(SELECT FROM api_bridge_always_grants g WHERE g.owner_id=a.owner_id AND g.enrollment_id=a.enrollment_id) THEN 'always'
        WHEN EXISTS(SELECT FROM api_bridge_session_grants g WHERE (g.owner_id,g.workspace_id,g.chat_id,g.enrollment_id)=(a.owner_id,a.workspace_id,a.chat_id,a.enrollment_id)) THEN 'session' END
        FROM api_bridge_tasks t JOIN api_bridge_computers USING(owner_id,enrollment_id) JOIN api_bridge_attachments a USING(owner_id,enrollment_id)
        WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND task_id=$5 AND expires_at>now() AND (dynamic OR ($6::text IS NULL AND $7::text IS NULL))")
        .bind(owner).bind(workspace).bind(chat).bind(&id).bind(input.task.to_string()).bind(&input.command).bind(&input.cwd).execute(&mut *tx).await?;
    if added.rows_affected() != 1 {
        return Err(ApiError::conflict("task_offline_or_detached"));
    }
    sqlx::query("INSERT INTO api_bridge_runs(request_id,task_id) VALUES($1::text::uuid,$2)")
        .bind(&id)
        .bind(input.task.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id})))
}
pub(super) async fn results(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat)): Path<(String, String)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    results_for(runtime(&state)?.shared_pool(), owner, workspace, chat).await
}
pub(super) async fn results_for(
    pool: sqlx::PgPool,
    owner: String,
    workspace: String,
    chat: String,
) -> Result<Json<Vec<Value>>, ApiError> {
    Ok(Json(sqlx::query_scalar("SELECT jsonb_build_object('id',request_id,'computer',computer_name,'command',command,'result',coalesce(result,jsonb_build_object('status',
        CASE WHEN claimed_at<now()-interval '80 seconds' THEN 'Outcome unknown' ELSE 'Running' END,'output',coalesce(progress,''))))
        FROM api_bridge_runs JOIN api_bridge_requests USING(request_id) WHERE owner_id=$1 AND workspace_id=$2 AND chat_id=$3 AND claimed ORDER BY created_at DESC,request_id LIMIT 30")
        .bind(owner).bind(workspace).bind(chat).fetch_all(&pool).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RuntimeRequest {
    chat_id: String,
    request: zuse_protocol::computer_tasks::Request,
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
    use zuse_protocol::computer_tasks::Request as Task;
    let result = match input.request {
        Task::List => json!(list_for(pool, owner, workspace, input.chat_id).await?.0),
        Task::Run { task, request_id } => {
            request_for(
                pool,
                owner,
                workspace,
                input.chat_id,
                Request {
                    task,
                    request_id,
                    command: None,
                    cwd: None,
                },
            )
            .await?
            .0
        }
        Task::Command {
            device_id,
            command,
            cwd,
            request_id,
        } => {
            let task:String=sqlx::query_scalar("SELECT task_id FROM api_bridge_tasks WHERE owner_id=$1 AND enrollment_id=$2 AND dynamic AND expires_at>now() ORDER BY expires_at DESC LIMIT 1")
                .bind(&owner).bind(device_id).fetch_optional(&pool).await?.ok_or_else(||ApiError::conflict("computer_offline"))?;
            request_for(
                pool,
                owner,
                workspace,
                input.chat_id,
                Request {
                    task: uuid::Uuid::parse_str(&task)
                        .map_err(|_| ApiError::conflict("invalid_task"))?,
                    request_id,
                    command: Some(command),
                    cwd,
                },
            )
            .await?
            .0
        }
        Task::Results => json!(results_for(pool, owner, workspace, input.chat_id).await?.0),
    };
    Ok(Json(result))
}

pub(super) async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace, chat, request)): Path<(String, String, uuid::Uuid)>,
) -> Result<StatusCode, ApiError> {
    let owner = personal_owner(&state, &headers).await?;
    sqlx::query("UPDATE api_bridge_runs b SET claimed=true, result=jsonb_build_object('status','Cancelled','output','') FROM api_bridge_requests r WHERE r.request_id=b.request_id AND r.owner_id=$1 AND r.workspace_id=$2 AND r.chat_id=$3 AND r.request_id=$4::text::uuid AND b.result IS NULL")
        .bind(owner).bind(workspace).bind(chat).bind(request.to_string()).execute(&runtime(&state)?.shared_pool()).await?;
    Ok(StatusCode::NO_CONTENT)
}
