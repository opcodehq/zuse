use super::*;
use axum::{
    Form, Json,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub(super) fn verify(
    secret: &str,
    headers: &HeaderMap,
    body: &[u8],
    seconds: i64,
) -> Result<(), ApiError> {
    let timestamp = headers
        .get("x-slack-request-timestamp")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(invalid)?;
    let parsed = timestamp.parse::<i64>().map_err(|_| invalid())?;
    if seconds.abs_diff(parsed) > 300 {
        return Err(ApiError::unauthorized("slack_request_expired"));
    }
    let signature = headers
        .get("x-slack-signature")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("v0="))
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(invalid)?;
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&signature[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| invalid())?;
    mac.update(format!("v0:{timestamp}:").as_bytes());
    mac.update(body);
    mac.verify_slice(&bytes)
        .map_err(|_| ApiError::unauthorized("invalid_slack_signature"))
}

pub(super) fn event(body: &Value, bot: &str) -> Option<Event> {
    if body["type"] != "event_callback" {
        return None;
    }
    let ev = &body["event"];
    if ev["bot_id"].as_str().is_some_and(|id| !id.is_empty())
        || ev["subtype"].as_str().is_some_and(|v| v != "file_share")
    {
        return None;
    }
    let string = |v: &Value| {
        v.as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .map(str::to_owned)
    };
    let team = string(&body["team_id"])?;
    let id = string(&body["event_id"])?;
    let channel = string(&ev["channel"])?;
    let user = string(&ev["user"])?;
    let ts = string(&ev["ts"])?;
    let text = ev["text"].as_str().unwrap_or("");
    if ev["type"] != "app_mention"
        && !(ev["type"] == "message"
            && ev["channel_type"] == "im"
            && (ev["thread_ts"].is_null() || text.contains(&format!("<@{bot}>"))))
    {
        return None;
    }
    let unsupported_files = ev["files"]
        .as_array()
        .is_some_and(|files| !files.is_empty());
    let too_large = text.len() > 40_000;
    Some(Event {
        team,
        id,
        channel,
        user,
        ts: ts.clone(),
        thread: string(&ev["thread_ts"]).unwrap_or(ts),
        text: if too_large {
            String::new()
        } else {
            text.into()
        },
        unsupported_files,
        too_large,
        generation: String::new(),
        revision: String::new(),
        source: None,
        workspace: None,
        session: None,
        progress_ts: None,
        replacement_for: None,
        result: None,
        submitted_at: None,
        context: vec![],
        context_cursor: None,
    })
}
pub(super) async fn events(
    State(s): State<Slack>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    verify(&s.config.signing_secret, &headers, &body, now() / 1000)?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| invalid())?;
    if value["type"] == "url_verification" {
        return Ok(Json(
            json!({"challenge":value["challenge"].as_str().ok_or_else(invalid)?}),
        ));
    }
    if value["event"]["type"] == "app_uninstalled" {
        if let Some(team) = value["team_id"].as_str() {
            let mut tx = s.pool.begin().await?;
            sqlx::query("DELETE FROM zuse_rust_slack_members WHERE team_id=$1")
                .bind(team)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM zuse_rust_slack_installs WHERE team_id=$1")
                .bind(team)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        return Ok(Json(json!({})));
    }
    let installation: Option<(String, String)> = sqlx::query_as(
        "SELECT generation,bot_user_id FROM zuse_rust_slack_installs WHERE team_id=$1",
    )
    .bind(value["team_id"].as_str().unwrap_or(""))
    .fetch_optional(&s.pool)
    .await?;
    let Some((generation, bot)) = installation else {
        return Ok(Json(json!({})));
    };
    if let Some(mut event) = event(&value, &bot) {
        event.generation = generation;
        event.revision = sqlx::query_scalar(
            "SELECT revision FROM zuse_rust_slack_members WHERE team_id=$1 AND user_id=$2",
        )
        .bind(&event.team)
        .bind(&event.user)
        .fetch_optional(&s.pool)
        .await?
        .unwrap_or_default();
        let payload = s.seal(&job_context(&event.team, &event.id), &event)?;
        sqlx::query("INSERT INTO zuse_rust_slack_jobs(team_id,event_id,payload,next_at,created_at,channel_id,message_ts,thread_ts) VALUES($1,$2,$3,$4,$4,$5,$6,$7) ON CONFLICT DO NOTHING").bind(&event.team).bind(&event.id).bind(payload).bind(now()).bind(&event.channel).bind(&event.ts).bind(&event.thread).execute(&s.pool).await?;
    }
    Ok(Json(json!({})))
}

#[derive(Serialize, Deserialize)]
struct Pending {
    member: Member,
}
#[derive(Deserialize)]
pub(super) struct OAuthQuery {
    state: String,
    code: Option<String>,
}
#[derive(Deserialize)]
pub(super) struct StartQuery {
    state: String,
}
fn cookie(value: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(&format!(
        "zuse_slack={value}; Path=/slack/oauth; HttpOnly; Secure; SameSite=Lax; Max-Age=600"
    ))
    .map_err(|_| invalid())
}
fn public_base(s: &Slack) -> String {
    s.config
        .redirect_uri
        .trim_end_matches("/slack/oauth/callback")
        .into()
}

pub(super) async fn connections(
    State(s): State<Slack>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    let rows:Vec<(String,String,String,Value)>=sqlx::query_as("SELECT m.team_id,i.team_name,m.user_id,m.settings FROM zuse_rust_slack_members m JOIN zuse_rust_slack_installs i USING(team_id) WHERE m.account_id=$1 AND m.actor_id=$2 ORDER BY i.team_name").bind(&access.owner_id).bind(&access.actor.account_id).fetch_all(&s.pool).await?;
    let connections=rows.into_iter().map(|(team,name,user,settings)|json!({"teamId":team,"teamName":name,"slackUserId":user,"projectId":settings["projectId"],"agent":settings["agent"],"model":settings["model"]})).collect::<Vec<_>>();
    let mut workspaces = vec![json!({"id":"personal","name":"Personal"})];
    if s.app.organizations_enabled {
        for id in s.app.organization_ids.iter() {
            match s
                .app
                .identity
                .membership(&access.actor.account_id, id)
                .await
            {
                Ok(m)
                    if m.status == "active"
                        && matches!(m.role.slug.as_str(), "member" | "admin") =>
                {
                    workspaces.push(json!({"id":format!("organization:{id}"),"name":s.app.identity.organization_name(id).await?}))
                }
                Ok(_) => {}
                Err(e) if e.status == axum::http::StatusCode::FORBIDDEN => {}
                Err(e) => return Err(e),
            }
        }
    }
    let workspace_id = match &access.scope {
        crate::access::Scope::Personal => "personal".to_owned(),
        crate::access::Scope::Organization { organization_id } => {
            format!("organization:{organization_id}")
        }
    };
    Ok(Json(
        json!({"workspaceId":workspace_id,"executionAvailable":access.membership.is_none(),"workspaces":workspaces,"connections":connections,"projects":s.app.store.launch_projects(&access.owner_id).await?,"catalog":crate::model_catalog::launch_catalog()}),
    ))
}
pub(super) async fn connect(
    State(s): State<Slack>,
    headers: HeaderMap,
    Json(settings): Json<ConnectRequest>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    if let Some(workspace) = &settings.workspace_id {
        let expected = match &access.scope {
            crate::access::Scope::Personal => "personal".to_owned(),
            crate::access::Scope::Organization { organization_id } => {
                format!("organization:{organization_id}")
            }
        };
        if *workspace != expected {
            return Err(ApiError::forbidden("slack_workspace_mismatch"));
        }
    }
    if access.membership.is_some() {
        return Err(ApiError::conflict("slack_organization_runtime_unavailable"));
    }
    let catalog = crate::model_catalog::launch_catalog();
    if !catalog
        .providers
        .iter()
        .any(|p| String::from(p.id) == settings.agent && p.resolve_model(&settings.model).is_some())
    {
        return Err(ApiError::bad_request("slack_model_unavailable"));
    }
    if !s
        .app
        .store
        .launch_projects(&access.owner_id)
        .await?
        .iter()
        .any(|p| {
            p["projectId"] == settings.project_id
                && p["prepared"] == true
                && p["state"] == "ready"
                && p["supportsSlack"] == true
        })
    {
        return Err(ApiError::conflict("slack_project_not_ready"));
    }
    let organization_id = match access.scope {
        crate::access::Scope::Personal => None,
        crate::access::Scope::Organization { organization_id } => Some(organization_id),
    };
    let state = nonce();
    let payload = s.seal(
        &format!("slack-oauth:{}", hash(&state)),
        &Pending {
            member: Member {
                account_id: access.owner_id,
                actor_id: access.actor.account_id,
                organization_id,
                settings,
                revision: nonce(),
            },
        },
    )?;
    sqlx::query("INSERT INTO zuse_rust_slack_oauth(state_hash,browser_hash,payload,expires_at) VALUES($1,'',$2,$3)").bind(hash(&state)).bind(payload).bind(now()+600_000).execute(&s.pool).await?;
    Ok(Json(
        json!({"url":format!("{}/slack/oauth/start?state={state}",public_base(&s))}),
    ))
}
pub(super) async fn start(
    State(s): State<Slack>,
    Query(q): Query<StartQuery>,
) -> Result<Response, ApiError> {
    let browser = nonce();
    let updated=sqlx::query("UPDATE zuse_rust_slack_oauth SET browser_hash=$2 WHERE state_hash=$1 AND browser_hash='' AND expires_at>$3").bind(hash(&q.state)).bind(hash(&browser)).bind(now()).execute(&s.pool).await?.rows_affected();
    if updated != 1 {
        return Err(ApiError::unauthorized("slack_oauth_expired"));
    }
    let payload: String =
        sqlx::query_scalar("SELECT payload FROM zuse_rust_slack_oauth WHERE state_hash=$1")
            .bind(hash(&q.state))
            .fetch_one(&s.pool)
            .await?;
    let pending: Pending = s.open(&format!("slack-oauth:{}", hash(&q.state)), &payload)?;
    let workspace = pending
        .member
        .organization_id
        .as_deref()
        .unwrap_or("Personal");
    let html = format!(
        "<!doctype html><html><head><meta name=viewport content='width=device-width'><title>Connect Slack</title></head><body><main><h1>Connect Slack to Zuse</h1><p>Zuse account: <strong>{}</strong></p><p>Workspace: {}<br>Repository: {}</p><p>Tasks from your Slack member will use this account’s repository access and cloud usage.</p><form method=post action='/slack/oauth/authorize'><input type=hidden name=state value='{}'><button type=submit>Choose Slack workspace</button></form><p>Close this page to cancel.</p></main></body></html>",
        escape(&pending.member.actor_id),
        escape(workspace),
        escape(&pending.member.settings.project_id),
        escape(&q.state)
    );
    let mut response = Html(html).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie(&browser)?);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    Ok(response)
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}
#[derive(Deserialize)]
pub(super) struct Authorize {
    state: String,
}
pub(super) async fn authorize(
    State(s): State<Slack>,
    headers: HeaderMap,
    Form(q): Form<Authorize>,
) -> Result<Response, ApiError> {
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(public_base(&s).as_str()) {
        return Err(ApiError::forbidden("slack_oauth_origin_mismatch"));
    }
    let browser = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|v| v.trim().strip_prefix("zuse_slack="))
        })
        .ok_or_else(|| ApiError::unauthorized("slack_oauth_browser_mismatch"))?;
    let updated=sqlx::query("UPDATE zuse_rust_slack_oauth SET authorized=true WHERE state_hash=$1 AND browser_hash=$2 AND authorized=false AND expires_at>$3").bind(hash(&q.state)).bind(hash(browser)).bind(now()).execute(&s.pool).await?.rows_affected();
    if updated != 1 {
        return Err(ApiError::unauthorized("slack_oauth_expired"));
    }
    let mut url =
        reqwest::Url::parse("https://slack.com/oauth/v2/authorize").map_err(|_| invalid())?;
    url.query_pairs_mut().append_pair("client_id",&s.config.client_id).append_pair("redirect_uri",&s.config.redirect_uri).append_pair("state",&q.state).append_pair("scope","app_mentions:read,chat:write,im:history,im:read,users:read,channels:read,channels:history,groups:read,groups:history");
    Ok(Redirect::to(url.as_str()).into_response())
}
pub(super) async fn callback(
    State(s): State<Slack>,
    headers: HeaderMap,
    Query(q): Query<OAuthQuery>,
) -> Result<Response, ApiError> {
    let browser = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|v| v.trim().strip_prefix("zuse_slack="))
        })
        .ok_or_else(|| ApiError::unauthorized("slack_oauth_browser_mismatch"))?;
    let payload:Option<String>=sqlx::query_scalar("DELETE FROM zuse_rust_slack_oauth WHERE state_hash=$1 AND browser_hash=$2 AND authorized=true AND expires_at>$3 RETURNING payload").bind(hash(&q.state)).bind(hash(browser)).bind(now()).fetch_optional(&s.pool).await?;
    let pending: Pending = s.open(
        &format!("slack-oauth:{}", hash(&q.state)),
        &payload.ok_or_else(|| ApiError::unauthorized("slack_oauth_expired"))?,
    )?;
    s.member_access(&pending.member).await?;
    let code = q
        .code
        .ok_or_else(|| ApiError::bad_request("slack_oauth_cancelled"))?;
    let response = s
        .http
        .post(format!("{}/oauth.v2.access", s.api_origin))
        .form(&[
            ("client_id", s.config.client_id.as_str()),
            ("client_secret", s.config.client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", s.config.redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|_| unavailable())?;
    if !response.status().is_success() {
        return Err(unavailable());
    }
    let value: Value = serde_json::from_slice(&crate::auth::bounded_body(response).await?)
        .map_err(|_| unavailable())?;
    if value["ok"] != true {
        return Err(unavailable());
    }
    let field = |v: &Value| {
        v.as_str()
            .filter(|v| !v.is_empty() && v.len() < 4096)
            .map(str::to_owned)
            .ok_or_else(unavailable)
    };
    let team = field(&value["team"]["id"])?;
    let team_name = field(&value["team"]["name"])?;
    let user = field(&value["authed_user"]["id"])?;
    let bot = field(&value["bot_user_id"])?;
    let token = field(&value["access_token"])?;
    let credentials = s.seal(&format!("slack-install:{team}"), &json!({"token":token}))?;
    let mut tx = s.pool.begin().await?;
    sqlx::query("INSERT INTO zuse_rust_slack_installs(team_id,team_name,bot_user_id,credentials,generation) VALUES($1,$2,$3,$4,$5) ON CONFLICT(team_id) DO UPDATE SET team_name=EXCLUDED.team_name,bot_user_id=EXCLUDED.bot_user_id,credentials=EXCLUDED.credentials,generation=CASE WHEN zuse_rust_slack_installs.bot_user_id=EXCLUDED.bot_user_id THEN zuse_rust_slack_installs.generation ELSE EXCLUDED.generation END").bind(&team).bind(team_name).bind(bot).bind(credentials).bind(nonce()).execute(&mut *tx).await?;
    let member = pending.member;
    sqlx::query("INSERT INTO zuse_rust_slack_members(team_id,user_id,account_id,actor_id,organization_id,settings,revision) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(team_id,user_id) DO UPDATE SET account_id=EXCLUDED.account_id,actor_id=EXCLUDED.actor_id,organization_id=EXCLUDED.organization_id,settings=EXCLUDED.settings,revision=EXCLUDED.revision").bind(&team).bind(&user).bind(member.account_id).bind(member.actor_id).bind(member.organization_id).bind(serde_json::to_value(member.settings).map_err(|_|invalid())?).bind(member.revision).execute(&mut *tx).await?;
    tx.commit().await?;
    let mut response=Html("<!doctype html><html><head><meta name=viewport content='width=device-width'><title>Slack connected</title></head><body><main><h1>Slack connected</h1><p>Return to Zuse and refresh Slack settings. Mention Zuse in a channel or send a direct message to start a task. Mention it in the same thread to continue.</p></main></body></html>").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie("")?);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    Ok(response)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Disconnect {
    team_id: String,
    slack_user_id: String,
}
pub(super) async fn disconnect(
    State(s): State<Slack>,
    headers: HeaderMap,
    Json(q): Json<Disconnect>,
) -> Result<Json<Value>, ApiError> {
    let access = s.access(&headers).await?;
    sqlx::query("DELETE FROM zuse_rust_slack_members WHERE team_id=$1 AND user_id=$2 AND account_id=$3 AND actor_id=$4").bind(q.team_id).bind(q.slack_user_id).bind(access.owner_id).bind(access.actor.account_id).execute(&s.pool).await?;
    Ok(Json(json!({"disconnected":true})))
}
