//! Authenticated account event connection. Tickets never enter URLs and are
//! consumed once. Every replay is filtered through current workspace permissions.
use super::*;
use crate::{
    access::Scope,
    events::{
        hub::{Hub as EventHub, PROTOCOL},
        tickets::Pending,
    },
};
use axum::extract::ws::{Message, WebSocket};
use futures::SinkExt;
use std::{collections::HashSet, time::Duration};
use tokio::sync::OwnedSemaphorePermit;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TicketRequest {
    device_id: uuid::Uuid,
    cursor: Option<String>,
}
pub(super) async fn ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TicketRequest>,
) -> Result<Json<Value>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    let hub = state
        .store
        .events
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("account_events_disabled"))?;
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .map(|(_, v)| v)
        .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
    if let Some(cursor) = &body.cursor {
        hub.cursors
            .decode(&access.actor.account_id, &access.owner_id, cursor, now())?;
    }
    let organization = match access.scope {
        Scope::Personal => None,
        Scope::Organization { organization_id } => Some(organization_id),
    };
    let ticket = hub
        .tickets
        .issue(
            bearer.to_owned(),
            access.actor.account_id,
            access.owner_id,
            organization,
            body.device_id,
            body.cursor,
        )
        .await?;
    Ok(Json(
        json!({"protocol":PROTOCOL,"ticket":ticket,"expiresInMs":20000}),
    ))
}
pub(super) async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    origins: Arc<Vec<HeaderValue>>,
) -> Result<Response, ApiError> {
    if headers
        .get(header::ORIGIN)
        .is_some_and(|origin| !origins.contains(origin))
    {
        return Err(ApiError::forbidden("account_event_origin_rejected"));
    }
    let values: Vec<_> = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .collect();
    if values.len() != 2 || values[0] != PROTOCOL {
        return Err(ApiError::unauthorized("account_event_ticket_rejected"));
    }
    let hub = state
        .store
        .events
        .clone()
        .ok_or_else(|| ApiError::unavailable("account_events_disabled"))?;
    let permit = hub
        .capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::unavailable("account_event_capacity"))?;
    let pending = hub
        .tickets
        .take(
            values[1]
                .strip_prefix("auth.")
                .ok_or_else(|| ApiError::unauthorized("account_event_ticket_rejected"))?,
        )
        .await?;
    // Consume and recheck before switching protocols, including membership and
    // native GitHub enrollment. A ticket is not a durable authorization grant.
    authorize(&state, &pending).await?;
    Ok(ws
        .protocols([PROTOCOL])
        .max_message_size(4096)
        .max_frame_size(4096)
        .on_upgrade(move |socket| serve(state, hub, pending, socket, permit)))
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
async fn authorize(state: &AppState, p: &Pending) -> Result<Access, ApiError> {
    // Signature/expiry is checked on every batch. Organization authority coalesces
    // bounded fresh membership checks across this device's tasks and event feed.
    let actor = state.identity.verify(p.bearer.as_str()).await?;
    if actor.account_id != p.actor {
        return Err(ApiError::forbidden("account_event_scope_changed"));
    }
    if p.organization.is_some() {
        if let Some(control) = state
            .runtime
            .as_ref()
            .filter(|r| r.scope_authority_configured())
        {
            let access = control.scope_actor(&p.owner, &p.actor).await?;
            if access.owner_id != p.owner {
                return Err(ApiError::forbidden("account_event_scope_changed"));
            }
            return Ok(access);
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", p.bearer.as_str()))
            .map_err(|_| ApiError::unauthorized("account_event_ticket_rejected"))?,
    );
    if let Some(org) = &p.organization {
        headers.insert(
            "x-zuse-workspace",
            HeaderValue::from_str(&format!("organization:{org}"))
                .map_err(|_| ApiError::forbidden("workspace_access_denied"))?,
        );
    }
    let access = state.access(&headers, AccessKind::Content).await?;
    if access.actor.account_id != p.actor || access.owner_id != p.owner {
        return Err(ApiError::forbidden("account_event_scope_changed"));
    }
    Ok(access)
}
async fn send(socket: &mut WebSocket, value: Value) -> Result<(), ApiError> {
    let bytes = serde_json::to_string(&value)
        .map_err(|_| ApiError::unavailable("account_event_encoding"))?;
    if bytes.len() > 1024 * 1024 {
        return Err(ApiError::unavailable("account_event_batch_too_large"));
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.send(Message::Text(bytes.into())),
    )
    .await
    .map_err(|_| ApiError::unavailable("account_event_backpressure"))?
    .map_err(|_| ApiError::unavailable("account_event_disconnected"))
}
async fn acknowledge(socket: &mut WebSocket, cursor: &str) -> Result<(), ApiError> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Text(value))) => {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Ack {
                        r#type: String,
                        cursor: String,
                    }
                    let ack: Ack = serde_json::from_str(&value)
                        .map_err(|_| ApiError::bad_request("invalid_account_event_ack"))?;
                    return if ack.r#type == "ack" && ack.cursor == cursor {
                        Ok(())
                    } else {
                        Err(ApiError::bad_request("invalid_account_event_ack"))
                    };
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return Err(ApiError::unavailable("account_event_disconnected")),
            }
        }
    })
    .await
    .map_err(|_| ApiError::unavailable("account_event_ack_timeout"))?
}
async fn visible(
    state: &AppState,
    access: &Access,
    ids: &[String],
) -> Result<HashSet<String>, ApiError> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let rows:Vec<(String,Value)>=sqlx::query_as("SELECT workspace_id,request_config FROM api_cloud_workspaces WHERE account_id=$1 AND workspace_id=ANY($2)").bind(&access.owner_id).bind(ids).fetch_all(&state.store.pool).await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, config)| {
            access
                .workspace_permission(&access.owner_id, &config)
                .is_ok()
                .then_some(id)
        })
        .collect())
}
// Cursor and visible-resource baseline share one database snapshot. Otherwise a
// create/delete between separate reads could be skipped by a fresh-device cursor.
async fn baseline(
    state: &AppState,
    access: &Access,
    p: &Pending,
) -> Result<(i64, Option<i64>, HashSet<String>), ApiError> {
    let mut tx = state.store.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let latest:i64=sqlx::query_scalar("SELECT COALESCE((SELECT next_sequence-1 FROM zuse_rust_account_event_cursors WHERE owner_id=$1),0)").bind(&p.owner).fetch_one(&mut *tx).await?;
    let personal = if p.owner != p.actor {
        Some(sqlx::query_scalar::<_,i64>("SELECT COALESCE((SELECT next_sequence-1 FROM zuse_rust_account_event_cursors WHERE owner_id=$1),0)").bind(&p.actor).fetch_one(&mut *tx).await?)
    } else {
        None
    };
    let rows:Vec<(String,Value)>=sqlx::query_as("SELECT workspace_id,jsonb_build_object('sharingPolicy',request_config->'sharingPolicy') FROM api_cloud_workspaces WHERE account_id=$1 ORDER BY workspace_id LIMIT 4097").bind(&p.owner).fetch_all(&mut *tx).await?;
    if rows.len() > 4096 {
        return Err(ApiError::conflict("account_event_scope_resync_required"));
    }
    let known = rows
        .into_iter()
        .filter_map(|(id, config)| {
            access
                .workspace_permission(&access.owner_id, &config)
                .is_ok()
                .then_some(id)
        })
        .collect();
    tx.commit().await?;
    Ok((latest, personal, known))
}
fn checkpoint(
    hub: &EventHub,
    p: &Pending,
    position: i64,
    personal: Option<i64>,
) -> Result<String, ApiError> {
    hub.cursors
        .encode_pair(&p.actor, &p.owner, position, personal, now())
}
async fn serve(
    state: AppState,
    hub: Arc<EventHub>,
    p: Pending,
    mut socket: WebSocket,
    _permit: OwnedSemaphorePermit,
) {
    let (device_lease, mut replaced) = hub.claim_device(&p.actor, p.device_id).await;
    let room = hub.room(&p.owner).await;
    let mut changed = room.changed.subscribe();
    let personal_room = if p.owner != p.actor {
        Some(hub.room(&p.actor).await)
    } else {
        None
    };
    let mut personal_changed = personal_room.as_ref().map(|room| room.changed.subscribe());
    let result:Result<(),ApiError>=async{
        let access=authorize(&state,&p).await?;
        let (latest,personal_latest,mut known)=baseline(&state,&access,&p).await?;
        let (mut position,mut personal)=match &p.cursor{
            Some(c)=>{let (position,prior)=hub.cursors.decode_pair(&p.actor,&p.owner,c,now())?;(position,personal_latest.map(|latest|prior.unwrap_or(latest)))},
            None=>(latest,personal_latest),
        };
        let cursor=checkpoint(&hub,&p,position,personal)?;
        send(&mut socket,json!({"type":"ready","cursor":cursor,"revalidate":true})).await?;acknowledge(&mut socket,&cursor).await?;
        let mut recheck=tokio::time::interval(Duration::from_secs(10));recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        hub.wake();
        loop{
            let _=*changed.borrow_and_update();if let Some(changed)=&mut personal_changed{let _=*changed.borrow_and_update();}
            let (replay,personal_replay)=tokio::try_join!(hub.events.replay_limit(&p.owner,position,64),async{
                match personal{
                    Some(position)=>hub.events.replay_limit(&p.actor,position,64).await,
                    None=>Ok(crate::events::store::Replay{events:vec![],latest:0,reset_required:false}),
                }
            })?;
            if replay.reset_required||personal_replay.reset_required{
                let access=authorize(&state,&p).await?;
                let (latest,personal_latest,baseline)=baseline(&state,&access,&p).await?;
                let cursor=checkpoint(&hub,&p,latest,personal_latest)?;
                send(&mut socket,json!({"type":"reset","cursor":cursor,"cacheRevoked":true})).await?;acknowledge(&mut socket,&cursor).await?;
                position=latest;personal=personal_latest;known=baseline;
            }else if !replay.events.is_empty()||!personal_replay.events.is_empty(){
                let access=authorize(&state,&p).await?;
                let ids:Vec<String>=replay.events.iter().filter_map(|e|e.workspace_id.clone()).collect();
                let allowed=visible(&state,&access,&ids).await?;
                let mut events=Vec::new();let mut revoked=Vec::new();
                for (source,owner) in [(&replay.events,&p.owner),(&personal_replay.events,&p.actor)]{
                    for event in source{
                        // Organization context may use an explicitly selected personal
                        // connection. Its private tasks/history never enter this feed.
                        if owner!=&p.owner&&!matches!(event.kind.as_str(),"connection"|"authentication"|"github"){continue;}
                        if event.payload["actorId"].as_str().is_some_and(|actor|actor!=p.actor){continue;}
                        if let Some(id)=&event.workspace_id{
                            if !allowed.contains(id){if known.remove(id){revoked.push(id.clone());}continue;}
                            known.insert(id.clone());
                        }
                        events.push(json!({"kind":event.kind,"ownerId":owner,"resourceId":event.resource_id,"workspaceId":event.workspace_id,"payload":event.payload,"createdAt":event.created_at}));
                    }
                }
                let next=replay.events.last().map_or(position,|e|e.sequence);
                let personal_next=personal.map(|prior|personal_replay.events.last().map_or(prior,|e|e.sequence));
                let has_more=next<replay.latest||personal_next.is_some_and(|n|n<personal_replay.latest);
                let cursor=checkpoint(&hub,&p,next,personal_next)?;
                send(&mut socket,json!({"type":"events","events":events,"revokedWorkspaceIds":revoked,"cursor":cursor,"hasMore":has_more})).await?;
                acknowledge(&mut socket,&cursor).await?;position=next;personal=personal_next;
                if known.len()>4096{return Err(ApiError::conflict("account_event_scope_resync_required"));}
                if has_more{continue;}
            }
            tokio::select!{
                _=changed.changed()=>{},
                _=async{if let Some(changed)=&mut personal_changed{let _=changed.changed().await;}else{std::future::pending::<()>().await;}}=>{},
                _=replaced.changed()=>return Err(ApiError::conflict("account_event_device_replaced")),
                _=recheck.tick()=>{
                    let access=authorize(&state,&p).await?;let ids:Vec<_>=known.iter().cloned().collect();let allowed=visible(&state,&access,&ids).await?;
                    let revoked:Vec<_>=known.difference(&allowed).cloned().collect();
                    if !revoked.is_empty(){
                        let cursor=checkpoint(&hub,&p,position,personal)?;
                        send(&mut socket,json!({"type":"events","events":[],"revokedWorkspaceIds":revoked,"cursor":cursor,"hasMore":false})).await?;
                        acknowledge(&mut socket,&cursor).await?;known=allowed;
                    }
                },
                incoming=socket.recv()=>match incoming{Some(Ok(Message::Ping(_)|Message::Pong(_)))=>{},_=>return Err(ApiError::unavailable("account_event_disconnected")),}
            }
        }
    }.await;
    if let Err(error) = result {
        let clear = error.status == StatusCode::FORBIDDEN
            || error.status == StatusCode::UNAUTHORIZED
            || error.code == "account_event_scope_resync_required";
        let _=send(&mut socket,json!({"type":"error","code":error.code,"status":error.status.as_u16(),"cacheRevoked":clear})).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
    hub.release_device(&p.actor, p.device_id, device_lease)
        .await;
}
