//! One account event socket per host app, shared by UI views and mirror discovery.
//! Frames are invalidations only. They never replay tools or wake a computer.
use super::*;
use futures::{SinkExt, StreamExt};
use std::{collections::HashSet, sync::Mutex};
use tokio::sync::{Notify, broadcast};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_util::sync::CancellationToken;
use zuse_rpc::{RpcError, RpcReply};

const PROTOCOL: &str = "zuse.account-events.v1";
#[derive(Clone, PartialEq, Eq)]
struct Scope {
    actor: String,
    organization: Option<String>,
}
impl Scope {
    fn current(auth: &Auth) -> Option<Self> {
        Some(Self {
            actor: auth.user_id()?,
            organization: auth.state().org_id().map(str::to_owned),
        })
    }
    fn matches(&self, auth: &Auth) -> bool {
        Self::current(auth).as_ref() == Some(self)
    }
}
#[derive(Default)]
pub(super) struct Manager {
    active: Mutex<Option<(Scope, Arc<Feed>)>>,
}
pub(super) struct Feed {
    frames: broadcast::Sender<Value>,
    cancel: CancellationToken,
    retry: Notify,
}
impl Drop for Feed {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Manager {
    pub(super) fn subscribe(
        &self,
        api: Arc<CloudApi>,
        auth: Auth,
    ) -> Result<(Arc<Feed>, broadcast::Receiver<Value>)> {
        let feed = self.get(api, auth)?;
        let receiver = feed.frames.subscribe();
        Ok((feed, receiver))
    }
    fn get(&self, api: Arc<CloudApi>, auth: Auth) -> Result<Arc<Feed>> {
        let scope = Scope::current(&auth)
            .ok_or_else(|| anyhow::anyhow!("Sign in to access cloud events."))?;
        let mut active = self
            .active
            .lock()
            .map_err(|_| anyhow::anyhow!("Cloud event manager unavailable."))?;
        if let Some((current, feed)) = &*active {
            if current == &scope && !feed.cancel.is_cancelled() {
                return Ok(feed.clone());
            }
            feed.cancel.cancel();
        }
        let feed = Arc::new(Feed {
            frames: broadcast::channel(256).0,
            cancel: CancellationToken::new(),
            retry: Notify::new(),
        });
        let weak = Arc::downgrade(&feed);
        let cancel = feed.cancel.clone();
        let run_scope = scope.clone();
        tokio::spawn(async move {
            run(api, auth, run_scope, weak, cancel).await;
        });
        *active = Some((scope, feed.clone()));
        Ok(feed)
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        if let Ok(active) = self.active.get_mut() {
            if let Some((_, feed)) = active {
                feed.cancel.cancel();
            }
        }
    }
}
fn envelope(scope: &Scope, value: Value) -> Value {
    json!({"accountId":scope.actor,"organizationId":scope.organization,"data":value})
}
pub async fn handle(auth: &Auth, params: Value) -> Result<RpcReply, RpcError> {
    let api = CloudApi::shared().map_err(|e| RpcError::Failed(e.to_string()))?;
    handle_with_api(auth, params, api).await
}
pub(crate) async fn handle_with_api(
    auth: &Auth,
    params: Value,
    api: Arc<CloudApi>,
) -> Result<RpcReply, RpcError> {
    let scope = Scope::current(auth)
        .ok_or_else(|| RpcError::Failed("Sign in to access cloud events.".into()))?;
    if params["expectedAccountId"].as_str() != Some(scope.actor.as_str())
        || params["expectedOrganizationId"].as_str() != scope.organization.as_deref()
    {
        return Err(RpcError::Failed(
            "The cloud account changed. Reopen this view.".into(),
        ));
    }
    let feed = api
        .account_events
        .get(api.clone(), auth.clone())
        .map_err(|e| RpcError::Failed(e.to_string()))?;
    let receiver = feed.frames.subscribe();
    feed.retry.notify_one();
    let initial = envelope(&scope, json!({"type":"revalidate","cacheRevoked":false}));
    let stream = futures::stream::unfold(
        (feed, receiver, Some(initial), scope),
        |(feed, mut receiver, initial, scope)| async move {
            if let Some(initial) = initial {
                return Some((initial, (feed, receiver, None, scope)));
            }
            let frame = match receiver.recv().await {
                Ok(frame) => frame,
                Err(broadcast::error::RecvError::Lagged(_)) => envelope(
                    &scope,
                    json!({"type":"reset","cacheRevoked":true,"reason":"consumer_lagged"}),
                ),
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            Some((frame, (feed, receiver, None, scope)))
        },
    )
    .boxed();
    Ok(RpcReply::Stream(stream))
}
fn publish(feed: &std::sync::Weak<Feed>, scope: &Scope, value: Value) {
    if let Some(feed) = feed.upgrade() {
        let _ = feed.frames.send(envelope(scope, value));
    }
}
async fn run(
    api: Arc<CloudApi>,
    auth: Auth,
    scope: Scope,
    feed: std::sync::Weak<Feed>,
    cancel: CancellationToken,
) {
    let device = uuid::Uuid::new_v4();
    let mut cursor: Option<String> = None;
    let mut failures = 0u32;
    let mut changes = auth.watch_state();
    loop {
        if cancel.is_cancelled() || !scope.matches(&auth) {
            break;
        }
        let attempt = connect(&api, &auth, &scope, device, &mut cursor, &feed, &cancel);
        let result = tokio::select! {biased;
            _=cancel.cancelled()=>break,
            changed=changes.changed()=>{if changed.is_err(){break;} if !scope.matches(&auth){break;}continue;},
            result=attempt=>result,
        };
        if !scope.matches(&auth) {
            break;
        }
        match result {
            Ok(()) => failures = 0,
            Err(Failure::Reset) => {
                cursor = None;
                apply_cache_effect(&api, CacheEffect::Catalog).await;
                publish(
                    &feed,
                    &scope,
                    json!({"type":"reset","cacheRevoked":true,"reason":"cursor_expired"}),
                );
                continue;
            }
            Err(Failure::Denied) => {
                cursor = None;
                apply_cache_effect(&api, CacheEffect::ClearAll).await;
                publish(
                    &feed,
                    &scope,
                    json!({"type":"error","code":"cloud_event_access_denied","cacheRevoked":true}),
                );
                tokio::select! {_=cancel.cancelled()=>break,_=changes.changed()=>{},_=async{if let Some(f)=feed.upgrade(){f.retry.notified().await;}}=>{}};
                continue;
            }
            Err(Failure::Disabled) => {
                publish(&feed, &scope, json!({"type":"disabled"}));
                tokio::select! {_=cancel.cancelled()=>break,_=changes.changed()=>{},_=async{if let Some(f)=feed.upgrade(){f.retry.notified().await;}}=>{}};
                continue;
            }
            Err(Failure::Transient) => {
                failures = failures.saturating_add(1);
                publish(&feed, &scope, json!({"type":"reconnecting"}));
            }
        }
        let wait = Duration::from_millis(
            (250u64 << failures.min(7)).min(30_000) + u64::from(uuid::Uuid::new_v4().as_bytes()[0]),
        );
        tokio::select! {_=cancel.cancelled()=>break,_=changes.changed()=>{},_=tokio::time::sleep(wait)=>{}};
    }
    cancel.cancel();
    publish(&feed, &scope, json!({"type":"closed","cacheRevoked":true}));
}
enum Failure {
    Reset,
    Denied,
    Disabled,
    Transient,
}
async fn connect(
    api: &CloudApi,
    auth: &Auth,
    scope: &Scope,
    device: uuid::Uuid,
    cursor: &mut Option<String>,
    feed: &std::sync::Weak<Feed>,
    cancel: &CancellationToken,
) -> std::result::Result<(), Failure> {
    // Only a signed-out session is a denial. A refresh that could not reach
    // the network (offline, just woke from sleep) retries without touching
    // anything saved.
    let token = zeroize::Zeroizing::new(auth.access_token().await.map_err(|error| {
        let signed_out = matches!(
            error.downcast_ref::<crate::account::AccountError>(),
            Some(crate::account::AccountError::SignedOut | crate::account::AccountError::Changed)
        ) || matches!(
            error.downcast_ref::<zuse_rpc::TokenError>(),
            Some(zuse_rpc::TokenError::SignedOut)
        );
        if signed_out {
            Failure::Denied
        } else {
            Failure::Transient
        }
    })?);
    if !scope.matches(auth) {
        return Err(Failure::Denied);
    }
    let url = api
        .origin
        .join("/v1/cloud/events/ticket")
        .map_err(|_| Failure::Transient)?;
    let mut request = api
        .http
        .post(url)
        .bearer_auth(token.as_str())
        .json(&json!({"deviceId":device,"cursor":cursor}));
    if let Some(org) = &scope.organization {
        request = request.header("x-zuse-workspace", format!("organization:{org}"));
    }
    let response = request.send().await.map_err(|_| Failure::Transient)?;
    let status = response.status().as_u16();
    if response.content_length().is_some_and(|n| n > 8192) {
        return Err(Failure::Transient);
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| Failure::Transient)?;
        if body.len() + chunk.len() > 8192 {
            return Err(Failure::Transient);
        }
        body.extend_from_slice(&chunk);
    }
    let grant: Value = serde_json::from_slice(&body).map_err(|_| Failure::Transient)?;
    match status {
        409 => return Err(Failure::Reset),
        // An expired bearer; the next attempt presents a refreshed one.
        401 => return Err(Failure::Transient),
        403 => return Err(Failure::Denied),
        404 => return Err(Failure::Disabled),
        503 if grant["error"] == "account_events_disabled" => return Err(Failure::Disabled),
        200 => {}
        _ => return Err(Failure::Transient),
    }
    let ticket = grant["ticket"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(Failure::Transient)?;
    if grant["protocol"] != PROTOCOL || !scope.matches(auth) {
        return Err(Failure::Denied);
    }
    let mut socket_url = api
        .origin
        .join("/v1/cloud/events")
        .map_err(|_| Failure::Transient)?;
    socket_url
        .set_scheme(if api.loopback { "ws" } else { "wss" })
        .map_err(|_| Failure::Transient)?;
    let mut request = socket_url
        .as_str()
        .into_client_request()
        .map_err(|_| Failure::Transient)?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!("{PROTOCOL}, auth.{ticket}")
            .parse()
            .map_err(|_| Failure::Transient)?,
    );
    let (mut socket, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| Failure::Transient)?
    .map_err(|_| Failure::Transient)?;
    if response
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
        != Some(PROTOCOL)
    {
        return Err(Failure::Denied);
    }
    while let Some(frame) =
        tokio::select! {_=cancel.cancelled()=>return Ok(()),frame=socket.next()=>frame}
    {
        if !scope.matches(auth) {
            return Err(Failure::Denied);
        }
        match frame.map_err(|_| Failure::Transient)? {
            Message::Text(text) => {
                if text.len() > 1024 * 1024 {
                    return Err(Failure::Transient);
                }
                let mut value: Value =
                    serde_json::from_str(&text).map_err(|_| Failure::Transient)?;
                match value["type"].as_str() {
                    Some("events" | "reset" | "ready") => {
                        let next = value["cursor"]
                            .as_str()
                            .filter(|c| !c.is_empty() && c.len() <= 2048)
                            .ok_or(Failure::Transient)?
                            .to_owned();
                        validate(&value).map_err(|_| Failure::Transient)?;
                        // Cursor is host-internal. Only bounded invalidations cross
                        // the RPC boundary; late frames remain actor/scope tagged.
                        value
                            .as_object_mut()
                            .ok_or(Failure::Transient)?
                            .remove("cursor");
                        if value["type"] == "ready" {
                            value["type"] = json!("connected");
                        }
                        apply_cache_effect(api, frame_cache_effect(&value)).await;
                        publish(feed, scope, value);
                        tokio::time::timeout(
                            Duration::from_secs(10),
                            socket.send(Message::Text(
                                json!({"type":"ack","cursor":next}).to_string().into(),
                            )),
                        )
                        .await
                        .map_err(|_| Failure::Transient)?
                        .map_err(|_| Failure::Transient)?;
                        *cursor = Some(next);
                    }
                    Some("error") => {
                        let revoked = value["cacheRevoked"] == true;
                        let expired = value["status"] == 401;
                        let effect = frame_cache_effect(&value);
                        apply_cache_effect(api, effect).await;
                        if expired {
                            // The socket's bearer outlived its lifetime. That
                            // is not a loss of access: reconnect with a
                            // refreshed token, and don't tell views (or open
                            // computer streams) that access changed.
                            publish(feed, scope, json!({"type":"reconnecting"}));
                            return Err(Failure::Transient);
                        }
                        publish(feed, scope, value);
                        return Err(if revoked {
                            Failure::Denied
                        } else {
                            Failure::Transient
                        });
                    }
                    _ => return Err(Failure::Transient),
                }
            }
            Message::Ping(bytes) => {
                socket
                    .send(Message::Pong(bytes))
                    .await
                    .map_err(|_| Failure::Transient)?;
            }
            Message::Pong(_) => {}
            Message::Close(_) => return Err(Failure::Transient),
            _ => return Err(Failure::Transient),
        }
    }
    Err(Failure::Transient)
}
fn validate(frame: &Value) -> Result<()> {
    if frame["type"] == "ready" {
        ensure!(frame["revalidate"] == true, "Invalid cloud checkpoint");
        return Ok(());
    }
    if frame["type"] == "reset" {
        ensure!(frame["cacheRevoked"] == true, "Invalid cloud reset");
        return Ok(());
    }
    let events = frame["events"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid cloud event batch"))?;
    ensure!(events.len() <= 128, "Cloud event batch too large");
    let revoked = frame["revokedWorkspaceIds"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid cloud revocation batch"))?;
    ensure!(revoked.len() <= 4096, "Cloud revocation batch too large");
    let kinds: HashSet<_> = [
        "workspace",
        "conversation",
        "connection",
        "authentication",
        "github",
        "environment",
        "task-command-queue",
        "task-command-progress",
    ]
    .into_iter()
    .collect();
    for event in events {
        ensure!(
            event["kind"].as_str().is_some_and(|s| kinds.contains(s)),
            "Invalid cloud event kind"
        );
        ensure!(
            event["resourceId"].as_str().is_some_and(|s| !s.is_empty()
                && s.len() <= 512
                && !s.chars().any(char::is_control)),
            "Invalid cloud resource"
        );
    }
    for id in revoked {
        ensure!(
            id.as_str().is_some_and(|s| !s.is_empty()
                && s.len() <= 512
                && !s.chars().any(char::is_control)),
            "Invalid cloud revocation"
        );
    }
    Ok(())
}

/// What a feed frame or failure means for saved chat history on disk. Saved
/// history only ever paints provisionally: every open still makes a fresh,
/// authorized read, and that read forgets a workspace it is denied (403) or
/// finds deleted (404). So only a confirmed account-wide denial wipes it all.
#[derive(Debug, PartialEq, Eq)]
enum CacheEffect {
    Keep,
    /// Missed invalidations (cursor expired, server reset): drop the saved
    /// sidebar catalog, which the next list read rebuilds; keep history.
    Catalog,
    /// These workspaces were withdrawn; keep every other chat.
    Forget(Vec<String>),
    /// Access to the account's cloud was denied (403, or signed out).
    ClearAll,
}
fn frame_cache_effect(frame: &Value) -> CacheEffect {
    if frame["type"] == "error" {
        // 401 = the socket's bearer expired. Refresh fixes it; nothing was lost.
        return if frame["cacheRevoked"] == true && frame["status"] != 401 {
            CacheEffect::ClearAll
        } else {
            CacheEffect::Keep
        };
    }
    let revoked: Vec<String> = frame["revokedWorkspaceIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect();
    if !revoked.is_empty() {
        return CacheEffect::Forget(revoked);
    }
    if frame["cacheRevoked"] == true {
        return CacheEffect::Catalog;
    }
    CacheEffect::Keep
}
async fn apply_cache_effect(api: &CloudApi, effect: CacheEffect) {
    if api.hosted || effect == CacheEffect::Keep {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = tokio::task::spawn_blocking(move || match effect {
        CacheEffect::Keep => {}
        CacheEffect::Catalog => super::sidebar_cache::clear_all(),
        CacheEffect::Forget(ids) => {
            super::sidebar_cache::clear_all();
            for id in ids {
                super::forget_native_workspace_history(&id);
            }
        }
        CacheEffect::ClearAll => {
            let _ = super::clear_native_history_cache();
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The server re-verifies the socket's bearer every few seconds, so each
    /// access-token expiry ends the socket with a 401 `cacheRevoked` error.
    /// That used to wipe every saved chat, leaving nothing to paint after a
    /// restart. Only a real denial clears everything.
    #[test]
    fn an_expired_socket_bearer_keeps_saved_history_and_only_denial_clears_it() {
        let expired =
            json!({"type":"error","code":"invalid_workos_token","status":401,"cacheRevoked":true});
        assert_eq!(frame_cache_effect(&expired), CacheEffect::Keep);
        let denied = json!({"type":"error","code":"workspace_access_denied","status":403,"cacheRevoked":true});
        assert_eq!(frame_cache_effect(&denied), CacheEffect::ClearAll);
        let resync = json!({"type":"error","code":"account_event_scope_resync_required","status":409,"cacheRevoked":true});
        assert_eq!(frame_cache_effect(&resync), CacheEffect::ClearAll);
        let unavailable = json!({"type":"error","code":"x","status":503,"cacheRevoked":false});
        assert_eq!(frame_cache_effect(&unavailable), CacheEffect::Keep);
        let revoked = json!({"type":"events","events":[],"revokedWorkspaceIds":["gone"]});
        assert_eq!(
            frame_cache_effect(&revoked),
            CacheEffect::Forget(vec!["gone".into()])
        );
        let reset = json!({"type":"reset","cacheRevoked":true});
        assert_eq!(frame_cache_effect(&reset), CacheEffect::Catalog);
        let events = json!({"type":"events","events":[{"kind":"workspace","resourceId":"w"}],"revokedWorkspaceIds":[]});
        assert_eq!(frame_cache_effect(&events), CacheEffect::Keep);
        assert_eq!(
            frame_cache_effect(&json!({"type":"connected","revalidate":true})),
            CacheEffect::Keep
        );
    }
    #[test]
    fn account_invalidations_are_bounded_and_never_accept_command_frames() {
        let frame = json!({"type":"events","events":[{"kind":"workspace","resourceId":"w"}],"revokedWorkspaceIds":[]});
        assert!(validate(&frame).is_ok());
        let mut command = frame.clone();
        command["events"][0]["kind"] = json!("executeTool");
        assert!(validate(&command).is_err());
        let mut huge = frame.clone();
        huge["events"] = json!(vec![frame["events"][0].clone(); 129]);
        assert!(validate(&huge).is_err());
        let mut malformed = frame;
        malformed["revokedWorkspaceIds"] = json!(["w\nother"]);
        assert!(validate(&malformed).is_err());
        assert!(validate(&json!({"type":"reset","cacheRevoked":true})).is_ok());
        assert!(validate(&json!({"type":"reset","cacheRevoked":false})).is_err());
    }
}
