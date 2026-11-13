//! Explicit API-owned commands. Legacy guest RPC is never a fallback for a v2 task.
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use zuse_protocol::{
    cloud_assets::Asset, cloud_task_commands as wire, methods,
    session_commands::SessionCommandPayload,
};
pub(super) mod assets;

type ModeKey = (String, Option<String>, String);
#[derive(Default)]
pub(super) struct Manager {
    uploads: assets::Manager,
    modes: std::sync::Mutex<std::collections::HashMap<ModeKey, (bool, u32, i64)>>,
}
impl Manager {
    pub(super) fn hosted() -> Self {
        Self {
            uploads: assets::Manager::hosted(),
            modes: Default::default(),
        }
    }

    fn cached(&self, key: &ModeKey) -> Option<(bool, u32)> {
        self.modes
            .lock()
            .ok()?
            .get(key)
            .filter(|(_, _, at)| *at > chrono::Utc::now().timestamp_millis() - 10_000)
            .map(|(mode, version, _)| (*mode, *version))
    }
    fn remember(&self, key: ModeKey, mode: bool, version: u32) {
        if let Ok(mut rows) = self.modes.lock() {
            let now = chrono::Utc::now().timestamp_millis();
            rows.retain(|k, (_, _, at)| k.0 == key.0 && k.1 == key.1 && *at > now - 10_000);
            if rows.len() < 512 || rows.contains_key(&key) {
                rows.insert(key, (mode, version, now));
            }
        }
    }
}

/// What a message that failed for a known refusal reason (its execution's
/// `errorCode`) means for the user. `None` for failures with no such reason.
pub fn refusal_message(code: &str) -> Option<&'static str> {
    super::task_rejection(code)
}

pub const ASSET_PREFIX: &str = "cloud-asset://";
pub fn asset_ref(asset: &Asset) -> Result<String> {
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(asset)?);
    ensure!(encoded.len() <= 4096, "Invalid cloud attachment metadata.");
    Ok(format!(
        "{ASSET_PREFIX}{encoded}/{}",
        asset.original_name.replace(['/', '\\'], "_")
    ))
}
pub fn parse_asset_ref(path: &str) -> Result<Asset> {
    let encoded = path
        .strip_prefix(ASSET_PREFIX)
        .and_then(|v| v.split('/').next())
        .filter(|v| !v.is_empty() && v.len() <= 4096)
        .ok_or_else(|| {
            anyhow::anyhow!("Upload this attachment to the selected cloud computer first.")
        })?;
    let asset: Asset = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded)?)?;
    ensure!(
        !asset.asset_id.is_empty()
            && asset.size_bytes <= zuse_protocol::cloud_assets::MAX_BYTES
            && asset.original_name.len() <= 512
            && zuse_protocol::cloud_assets::supported_mime(&asset.mime_type),
        "Invalid cloud attachment metadata."
    );
    Ok(asset)
}
fn paths(value: &Value) -> Result<Vec<Asset>> {
    let rows = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Cloud attachments are unavailable."))?;
    ensure!(
        rows.len() <= zuse_protocol::cloud_assets::MAX_ASSETS,
        "Attach up to eight files."
    );
    rows.iter()
        .map(|v| {
            parse_asset_ref(
                v.as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid cloud attachment."))?,
            )
        })
        .collect()
}
fn string(p: &Value, key: &str) -> Result<String> {
    Ok(p[key]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 512)
        .ok_or_else(|| anyhow::anyhow!("Cloud command is missing {key}."))?
        .to_owned())
}
fn revision(p: &Value) -> Result<i64> {
    p["revision"].as_i64().filter(|r| *r > 0).ok_or_else(|| {
        anyhow::anyhow!("The queue changed. Refresh its current revision before changing it.")
    })
}
/// The admission query a `CloudTaskCommand` carries: a caller that shows
/// server-queued messages asks for a run that has to wait for its computer to
/// be queued instead of refused ([`QUEUE_WHILE_WAITING`]). Servers without it
/// ignore the flag and refuse as before.
///
/// [`QUEUE_WHILE_WAITING`]: zuse_protocol::cloud_runtime_updates::QUEUE_WHILE_WAITING
pub(super) fn submit_query(params: &Value) -> Vec<(String, String)> {
    use zuse_protocol::cloud_runtime_updates::QUEUE_WHILE_WAITING;
    if params[QUEUE_WHILE_WAITING] == true {
        vec![(QUEUE_WHILE_WAITING.into(), "true".into())]
    } else {
        Vec::new()
    }
}
pub fn is_command_method(name: &str) -> bool {
    matches!(
        name,
        methods::CREATE_THREAD_SESSION
            | methods::QUEUE_COMMAND
            | methods::QUEUE_MESSAGE
            | methods::UPDATE_QUEUED_MESSAGE
            | methods::BEGIN_QUEUED_MESSAGE_EDIT
            | methods::RENEW_QUEUED_MESSAGE_EDIT
            | methods::FINISH_QUEUED_MESSAGE_EDIT
            | methods::MOVE_QUEUED_MESSAGE
            | methods::REMOVE_QUEUED_MESSAGE
            | methods::SEND_QUEUED_MESSAGE_NOW
            | methods::STEER_QUEUED_MESSAGE_NOW
    )
}
/// Whether `path` has the shape of a file uploaded to a cloud computer
/// (`<data>/uploads/<id>-<name>`). The computer itself checks it is one of its
/// own uploads before a run reads it.
fn computer_upload_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.is_absolute()
        && path.as_os_str().len() <= 1024
        && path.components().all(|part| {
            matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
        && path
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == "uploads")
}
/// Splits a queued message's files: `cloud-asset://` refs are files a server
/// still holds, and absolute paths are files already uploaded straight to the
/// computer (a queued row lists both kinds under `attachments`).
fn queued_files(p: &Value) -> Result<(Vec<Asset>, Vec<String>)> {
    let mut computer: Vec<String> =
        serde_json::from_value(p.get("computerFiles").cloned().unwrap_or(json!([])))?;
    let mut assets = Vec::new();
    for path in p
        .get("attachments")
        .unwrap_or(&json!([]))
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Cloud attachments are unavailable."))?
    {
        let path = path
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid cloud attachment."))?;
        if path.starts_with(ASSET_PREFIX) {
            assets.push(Value::String(path.to_owned()));
        } else if path.starts_with('/') {
            if !computer.iter().any(|known| known == path) {
                computer.push(path.to_owned());
            }
        } else {
            parse_asset_ref(path)?;
        }
    }
    let assets = paths(&Value::Array(assets))?;
    ensure!(
        assets.len() + computer.len() <= zuse_protocol::cloud_assets::MAX_ASSETS,
        "Attach up to eight files."
    );
    Ok((assets, computer))
}
fn input(p: &Value) -> Result<wire::Input> {
    let (attachments, computer_files) = queued_files(p)?;
    Ok(wire::Input {
        run_selection: serde_json::from_value(
            p.get("runSelection").cloned().unwrap_or(Value::Null),
        )?,
        text: p["text"].as_str().unwrap_or_default().to_owned(),
        attachments,
        computer_files,
        file_refs: serde_json::from_value(p.get("fileRefs").cloned().unwrap_or(json!([])))?,
        skill_refs: serde_json::from_value(p.get("skillRefs").cloned().unwrap_or(json!([])))?,
        annotations: serde_json::from_value(p.get("annotations").cloned().unwrap_or(json!([])))?,
        as_goal: p["asGoal"] == true,
    })
}
pub(super) fn normalize(name: &str, p: &Value) -> Result<wire::Request> {
    let session_id = string(p, "chatId")?;
    let command_id = p["cloudCommandId"]
        .as_str()
        .or_else(|| p["commandId"].as_str())
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow::anyhow!("Cloud action identity is missing. Retry the original action.")
        })?;
    let action = match name {
        methods::CREATE_THREAD_SESSION => wire::Action::CreateSession {
            title: p["title"].as_str().map(str::to_owned),
        },
        methods::QUEUE_COMMAND => {
            let mut payload: SessionCommandPayload = serde_json::from_value(p["command"].clone())?;
            let attachments = if let SessionCommandPayload::Run { request, .. } = &mut payload {
                // Files already uploaded straight to the computer stay on the
                // run by their path there; `cloud-asset://` refs are files a
                // server still holds. Any other path (this device's) fails.
                let (computer, held): (Vec<String>, Vec<String>) =
                    std::mem::take(&mut request.attachments)
                        .into_iter()
                        .partition(|path| computer_upload_path(path));
                let assets = paths(&serde_json::to_value(&held)?)?;
                ensure!(
                    assets.len() + computer.len() <= zuse_protocol::cloud_assets::MAX_ASSETS,
                    "Attach up to eight files."
                );
                // Local cwd, worktree and MCP configuration are never guest authority.
                ensure!(
                    request.worktree.is_none() && request.mcp.is_none() && request.resume.is_none(),
                    "This cloud action requires a task-relative run."
                );
                request.prompt = zuse_protocol::message_attachments::parse(&request.prompt).text;
                ensure!(
                    request.cwd.len() <= 4096
                        && !request.cwd.contains('\\')
                        && !std::path::Path::new(&request.cwd).is_absolute()
                        && std::path::Path::new(&request.cwd)
                            .components()
                            .all(|part| matches!(
                                part,
                                std::path::Component::Normal(_) | std::path::Component::CurDir
                            )),
                    "Choose a task-relative working directory for this cloud command."
                );
                if request.cwd.is_empty() {
                    request.cwd = ".".into();
                }
                request.attachments = computer;
                assets
            } else {
                if let SessionCommandPayload::Steer { prompt, .. } = &payload {
                    ensure!(
                        zuse_protocol::message_attachments::parse(prompt)
                            .attachments
                            .is_empty(),
                        "Use Send now to deliver cloud attachments."
                    );
                }
                Vec::new()
            };
            let message_id = match &payload {
                SessionCommandPayload::Run { message_id, .. } => message_id.clone(),
                SessionCommandPayload::Steer {
                    message_id: Some(id),
                    ..
                } => id.clone(),
                _ => format!("message:{command_id}"),
            };
            wire::Action::Submit {
                message_id,
                payload,
                attachments,
                based_on: serde_json::from_value(p.get("basedOn").cloned().unwrap_or(Value::Null))?,
            }
        }
        methods::QUEUE_MESSAGE => wire::Action::Queue {
            queue_id: p["queueId"]
                .as_str()
                .or_else(|| p["messageId"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("message:{command_id}")),
            message_id: p["messageId"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("message:{command_id}")),
            input: input(p)?,
            hold_for_turn_end: p["holdForTurnEnd"] == true,
        },
        methods::UPDATE_QUEUED_MESSAGE => wire::Action::Edit {
            queue_id: string(p, "id")?,
            revision: revision(p)?,
            input: input(p)?,
            lease_id: None,
        },
        methods::REMOVE_QUEUED_MESSAGE => wire::Action::Delete {
            queue_id: string(p, "id")?,
            revision: revision(p)?,
            lease_id: p["leaseId"].as_str().map(str::to_owned),
        },
        methods::MOVE_QUEUED_MESSAGE => wire::Action::Move {
            queue_id: string(p, "id")?,
            revision: revision(p)?,
            to_index: p["toIndex"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid queue position."))?,
        },
        methods::SEND_QUEUED_MESSAGE_NOW => wire::Action::SendNow {
            queue_id: string(p, "id")?,
            revision: revision(p)?,
        },
        methods::STEER_QUEUED_MESSAGE_NOW => wire::Action::SteerNow {
            queue_id: string(p, "id")?,
            revision: revision(p)?,
            based_on: serde_json::from_value(p.get("basedOn").cloned().unwrap_or(Value::Null))?,
        },
        methods::BEGIN_QUEUED_MESSAGE_EDIT => wire::Action::BeginEdit {
            queue_id: string(p, "id")?,
            revision: revision(p)?,
            editor_device_id: string(p, "editorDeviceId")?,
            editor_instance_id: string(p, "editorInstanceId")?,
        },
        methods::RENEW_QUEUED_MESSAGE_EDIT => wire::Action::RenewEdit {
            queue_id: string(p, "id")?,
            lease_id: string(p, "leaseId")?,
        },
        methods::FINISH_QUEUED_MESSAGE_EDIT => match p["action"].as_str() {
            Some("cancel" | "releaseUnchanged") => wire::Action::CancelEdit {
                queue_id: string(p, "id")?,
                lease_id: string(p, "leaseId")?,
            },
            Some("discard") => wire::Action::Delete {
                queue_id: string(p, "id")?,
                revision: revision(p)?,
                lease_id: Some(string(p, "leaseId")?),
            },
            Some("commit") => wire::Action::Edit {
                queue_id: string(p, "id")?,
                revision: revision(p)?,
                input: input(p)?,
                lease_id: Some(string(p, "leaseId")?),
            },
            _ => anyhow::bail!("Invalid cloud queue edit action."),
        },
        _ => anyhow::bail!("Unsupported cloud command action."),
    };
    // Only an explicit foreground Run may request wake. Queue maintenance,
    // session creation and steering remain nonwaking even if a caller adds it.
    let wake = if matches!(
        &action,
        wire::Action::Submit {
            payload: SessionCommandPayload::Run { .. },
            ..
        }
    ) {
        serde_json::from_value::<Option<wire::WakePolicy>>(
            p.get("wake").cloned().unwrap_or(Value::Null),
        )?
        .unwrap_or_default()
    } else {
        wire::WakePolicy::PreserveSleep
    };
    Ok(wire::Request {
        command_id,
        session_id,
        action,
        wake,
    })
}
fn queue_shape(mut page: Value) -> Result<Value> {
    if let Some(rows) = page["items"].as_array_mut() {
        for row in rows {
            let assets: Vec<Asset> =
                serde_json::from_value(row.get("attachments").cloned().unwrap_or(json!([])))?;
            let mut files = assets.iter().map(asset_ref).collect::<Result<Vec<_>>>()?;
            // Files uploaded straight to the computer are listed by their path there.
            let computer: Vec<String> =
                serde_json::from_value(row.get("computerFiles").cloned().unwrap_or(json!([])))?;
            files.extend(computer);
            row["attachments"] = json!(files);
            row["issuedBy"] = json!("cloud-authority");
            row["issuedAt"] = row.get("createdAt").cloned().unwrap_or(json!(0));
            if !row["editing"].is_null() {
                row["deliveryGate"] = json!({"kind":"editing","leaseId":"server-lease-held","ownerDeviceId":"cloud-authority","ownerInstanceId":"cloud-authority","acquiredAtMs":0,"expiresAtMs":row["editing"]["expiresAt"],"baseTextHash":"unavailable"});
            }
        }
    }
    Ok(page)
}
fn adapt(name: &str, p: &Value, reply: Value, queue: Option<&Value>) -> Result<Value> {
    match name {
        methods::CREATE_THREAD_SESSION => {
            ensure!(
                reply["sessionId"] == p["chatId"]
                    && reply["threadId"].as_str().is_some_and(|id| !id.is_empty()),
                "Cloud session response does not match its requested identity."
            );
            let at = reply["createdAt"]
                .as_i64()
                .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                .ok_or_else(|| anyhow::anyhow!("Cloud session timestamp is unavailable."))?;
            let mut chat = p
                .get("templateChat")
                .filter(|c| c.is_object())
                .cloned()
                .unwrap_or(json!({}));
            chat["id"] = reply["sessionId"].clone();
            chat["threadId"] = reply["threadId"].clone();
            chat["createdAt"] = json!(at);
            chat["archived"] = json!(false);
            chat["cwd"] = json!(".");
            chat["title"] = p["title"].clone();
            chat["harnessSessionId"] = Value::Null;
            chat["harnessSessionCwd"] = Value::Null;
            chat["lastMessageAt"] = Value::Null;
            chat["lastMessagePreview"] = Value::Null;
            Ok(chat)
        }
        methods::BEGIN_QUEUED_MESSAGE_EDIT => {
            ensure!(
                reply["leaseId"].as_str().is_some_and(|id| !id.is_empty())
                    && reply["expiresAt"].as_i64().is_some_and(|n| n > 0)
                    && reply["revision"] == p["revision"],
                "The cloud edit lease was not confirmed."
            );
            let row = queue
                .and_then(|q| q["items"].as_array())
                .and_then(|rows| {
                    rows.iter()
                        .find(|r| r["id"] == p["id"] && r["revision"] == p["revision"])
                })
                .ok_or_else(|| {
                    anyhow::anyhow!("The queued text changed before editing. Refresh the queue.")
                })?;
            let text = row["text"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Queued text is unavailable."))?;
            Ok(
                json!({"outcome":"acquired","leaseId":reply["leaseId"],"expiresAtMs":reply["expiresAt"],"revision":reply["revision"],"text":text,"attachments":row["attachments"],"baseTextHash":format!("{:x}",Sha256::digest(text.as_bytes()))}),
            )
        }
        methods::RENEW_QUEUED_MESSAGE_EDIT => Ok(
            json!({"outcome":"renewed","expiresAtMs":reply["expiresAt"],"revision":reply["revision"]}),
        ),
        methods::FINISH_QUEUED_MESSAGE_EDIT => Ok(
            json!({"outcome":match p["action"].as_str(){Some("discard")=>"discarded",Some("cancel"|"releaseUnchanged")=>"cancelled",_=>"committed"},"revision":reply["revision"]}),
        ),
        methods::SEND_QUEUED_MESSAGE_NOW | methods::STEER_QUEUED_MESSAGE_NOW => Ok(
            json!({"sent":true,"commandId":reply["commandId"],"durablyAdmitted":reply["durablyAdmitted"]}),
        ),
        _ => Ok(reply),
    }
}

/// Controller-only signal after admission. Its consumer owns a passive status
/// subscription; constructing a weak Manager entry here would drop the socket.
pub(super) fn passive_attachment(
    auth: &Auth,
    account: &str,
    organization: Option<&str>,
    workspace: &str,
    request: &wire::Request,
    receipt: &Value,
) -> Result<Option<Value>> {
    scoped(auth, account, organization)?;
    if request.wake != wire::WakePolicy::ExplicitResume
        || !matches!(
            &request.action,
            wire::Action::Submit {
                payload: SessionCommandPayload::Run { .. },
                ..
            }
        )
        || receipt["durablyAdmitted"] != true
        || receipt["commandId"] != request.command_id
    {
        return Ok(None);
    }
    Ok(Some(
        json!({"mode":"passive","accountId":account,"organizationId":organization,
        "workspaceId":workspace,"sessionId":request.session_id,"commandId":request.command_id}),
    ))
}

fn scoped(auth: &Auth, account: &str, organization: Option<&str>) -> Result<()> {
    ensure!(
        auth.user_id().as_deref() == Some(account) && auth.state().org_id() == organization,
        "The cloud account changed. Reopen this computer."
    );
    Ok(())
}
async fn queue(
    api: &CloudApi,
    auth: &Auth,
    account: &str,
    organization: Option<&str>,
    workspace: &str,
    session: &str,
) -> Result<Value> {
    scoped(auth, account, organization)?;
    let token = auth.access_token().await?;
    scoped(auth, account, organization)?;
    let result = api
        .request_scoped(
            &token,
            &[
                "v1",
                "cloud",
                "workspaces",
                workspace,
                "task-commands",
                "queue",
            ],
            None,
            organization,
            &[("sessionId".into(), session.into())],
        )
        .await?;
    scoped(auth, account, organization)?;
    queue_shape(result)
}
fn queue_hint(frame: &Value, workspace: &str) -> bool {
    let data = &frame["data"];
    matches!(
        data["type"].as_str(),
        Some("ready" | "reset" | "revalidate")
    ) || data["events"].as_array().is_some_and(|events| {
        events.iter().any(|e| {
            e["kind"] == "task-command-queue"
                && (e["workspaceId"] == workspace || e["resourceId"] == workspace)
        })
    })
}
/// Uploads marked for the computer, and reads of a file already there (an
/// absolute path rather than a server `cloud-asset://` ref), go to the
/// computer itself instead of server asset storage.
pub(super) fn computer_file_call(name: &str, p: &Value) -> bool {
    match name {
        methods::UPLOAD_CHUNK | methods::UPLOAD_COMMIT => p["computer"] == true,
        methods::READ_ATTACHMENT_CHUNK => {
            p["path"].as_str().is_some_and(|path| path.starts_with('/'))
        }
        _ => false,
    }
}
/// Called before acquiring an execution environment. A sleeping guest stays asleep.
pub(super) async fn maybe_forward(
    api: Arc<CloudApi>,
    auth: Auth,
    account: String,
    organization: Option<String>,
    workspace: String,
    name: &str,
    p: Value,
    id: u64,
) -> Result<Option<zuse_rpc::RpcReply>, zuse_rpc::RpcError> {
    use futures::StreamExt;
    use zuse_rpc::{RpcError, RpcReply};
    // Files for a run, a queued message or a queued message's edit go straight
    // to the awake computer; the server never stores them, and they read back
    // from the computer. Server-held files of older clients still read back.
    if computer_file_call(name, &p) {
        return Ok(None);
    }
    let supported = is_command_method(name)
        || matches!(
            name,
            methods::WATCH_QUEUE
                | methods::UPLOAD_CHUNK
                | methods::UPLOAD_COMMIT
                | methods::READ_ATTACHMENT_CHUNK
                | methods::CLOUD_TASK_COMMAND
        );
    if !supported {
        return Ok(None);
    }
    let cleanup = (
        api.clone(),
        account.clone(),
        organization.clone(),
        workspace.clone(),
    );
    let run=async {
        scoped(&auth,&account,organization.as_deref())?;
        let token=auth.access_token().await?;scoped(&auth,&account,organization.as_deref())?;
        let mode_key=(account.clone(),organization.clone(),workspace.clone());
        let (v2,queue_selection_version)=if let Some(mode)=api.task_commands.cached(&mode_key){mode}else{
            let metadata=api.request_scoped(&token,&["v1","cloud","workspaces",&workspace],None,organization.as_deref(),&[]).await?;
            scoped(&auth,&account,organization.as_deref())?;
            let metadata=metadata.get("workspace").unwrap_or(&metadata);
            let mode=metadata["commandDeliveryMode"]==wire::MODE&&metadata["commandAdmissionVersion"]==wire::VERSION;
            ensure!(mode||metadata["commandDeliveryMode"].is_null()||metadata["commandDeliveryMode"]=="mailbox-v1","Cloud command mode is unsupported. Reopen this computer.");
            let version=metadata["queueRunSelectionVersion"].as_u64().unwrap_or(0) as u32;api.task_commands.remember(mode_key,mode,version);(mode,version)
        };
        if !v2 {
            ensure!(name!=methods::CLOUD_TASK_COMMAND&&p["commandDeliveryMode"]!=wire::MODE&&!p["path"].as_str().is_some_and(|v|v.starts_with(ASSET_PREFIX)),"This computer does not support server-admitted commands. Reopen its current configuration.");
            return Ok::<_,anyhow::Error>(None);
        }
        if name==methods::WATCH_QUEUE {
            let session=string(&p,"chatId")?;
            // Subscribe first; invalidations committed during the initial read remain queued.
            let (feed,receiver)=api.account_events.subscribe(api.clone(),auth.clone())?;
            let first=queue(&api,&auth,&account,organization.as_deref(),&workspace,&session).await?;
            let stream=futures::stream::unfold((api,auth,account,organization,workspace,session,feed,receiver,Some(first.clone()),first),move|(api,auth,account,organization,workspace,session,feed,mut receiver,initial,mut previous)|async move{
                if let Some(page)=initial{return Some((json!({"id":id,"item":page}),(api,auth,account,organization,workspace,session,feed,receiver,None,previous)));}
                loop{
                    let dirty=match receiver.recv().await{
                        Ok(frame)=>{
                            if frame["accountId"]!=account||frame["organizationId"].as_str()!=organization.as_deref(){continue;}
                            if frame["data"]["cacheRevoked"]==true||frame["data"]["revokedWorkspaceIds"].as_array().is_some_and(|ids|ids.iter().any(|v|v==&workspace)){
                                return Some((json!({"id":id,"err":"Cloud access changed. Reopen this computer."}),(api,auth,account,organization,workspace,session,feed,receiver,None,previous)));
                            }
                            queue_hint(&frame,&workspace)
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>true,
                        Err(_)=>return None,
                    };
                    if !dirty{continue;}
                    match queue(&api,&auth,&account,organization.as_deref(),&workspace,&session).await{
                        Ok(page)=>{if page==previous{continue;}previous=page.clone();return Some((json!({"id":id,"item":page}),(api,auth,account,organization,workspace,session,feed,receiver,None,previous)));}
                        Err(_)=>return Some((json!({"id":id,"err":"Cloud queue could not be revalidated. Reopen this computer."}),(api,auth,account,organization,workspace,session,feed,receiver,None,previous))),
                    }
                }
            }).boxed();
            return Ok(Some(RpcReply::Stream(stream)));
        }
        if matches!(name,methods::UPLOAD_CHUNK|methods::UPLOAD_COMMIT|methods::READ_ATTACHMENT_CHUNK){
            let reply=assets::forward(&api,&auth,&account,organization.as_deref(),&workspace,name,p).await?;
            return Ok(Some(RpcReply::Stream(futures::stream::once(async move{json!({"id":id,"ok":reply})}).boxed())));
        }
        let mut p=p;
        let needs_snapshot=name==methods::BEGIN_QUEUED_MESSAGE_EDIT||name==methods::UPDATE_QUEUED_MESSAGE||(name==methods::FINISH_QUEUED_MESSAGE_EDIT&&p["action"]=="commit");
        let before=if needs_snapshot {Some(queue(&api,&auth,&account,organization.as_deref(),&workspace,&string(&p,"chatId")?).await?)}else{None};
        if needs_snapshot {
            let row=before.as_ref().and_then(|q|q["items"].as_array()).and_then(|rows|rows.iter().find(|row|row["id"]==p["id"]&&row["revision"]==p["revision"])).ok_or_else(||anyhow::anyhow!("The queue changed. Review its current row before editing."))?;
            if name!=methods::BEGIN_QUEUED_MESSAGE_EDIT {for key in["attachments","fileRefs","skillRefs","annotations","asGoal","runSelection"]{if p.get(key).is_none(){if let Some(value)=row.get(key).or_else(||row["input"].get(key)){p[key]=value.clone();}}}}
        }
        let request=if name==methods::CLOUD_TASK_COMMAND {serde_json::from_value::<wire::Request>(p["request"].clone())?}else{normalize(name,&p)?};
        let selected_queue = match &request.action {wire::Action::Queue{input,..}|wire::Action::Edit{input,..}=>input.run_selection.is_some(),_=>false};
        ensure!(!selected_queue || queue_selection_version==wire::QUEUE_RUN_SELECTION_VERSION,"This server cannot save the selected agent with queued messages yet. Your draft is preserved; wait for the current turn to finish, then Send.");
        scoped(&auth,&account,organization.as_deref())?;
        let query=if name==methods::CLOUD_TASK_COMMAND {submit_query(&p)} else {Vec::new()};
        let result=api.request_scoped(&token,&["v1","cloud","workspaces",&workspace,"task-commands"],Some(serde_json::to_value(&request)?),organization.as_deref(),&query).await;
        // The exact typed body/ID is retained by this owned operation. A lost HTTP
        // response never becomes a different command or an opaque guest RPC.
        let result=match result {Ok(value)=>value,Err(original) if original.downcast_ref::<super::TaskRejection>().is_some()||original.downcast_ref::<super::unknown_outcome::UnknownOutcome>().is_some()=>return Err(original),Err(original)=>{
            scoped(&auth,&account,organization.as_deref())?;
            match api.request_scoped(&token,&["v1","cloud","workspaces",&workspace,"task-commands"],Some(serde_json::to_value(&request)?),organization.as_deref(),&query).await{
                Ok(value)=>value,Err(_)=>return Err(original),
            }
        }};
        scoped(&auth,&account,organization.as_deref())?;
        ensure!(result["durablyAdmitted"]==true&&result["commandId"]==request.command_id,"The cloud action did not return its durable admission identity. Check its original action before retrying.");
        let attachment=passive_attachment(&auth,&account,organization.as_deref(),&workspace,&request,&result)?;
        let mut response=adapt(name,&p,result,before.as_ref())?;
        if let Some(attachment)=attachment {response["passiveAttachment"]=attachment;}
        Ok(Some(RpcReply::Stream(futures::stream::once(async move{json!({"id":id,"ok":response})}).boxed())))
    }.await;
    if run.is_err() {
        cleanup
            .0
            .task_commands
            .uploads
            .revoke(&cleanup.1, cleanup.2.as_deref(), &cleanup.3);
    }
    run.map_err(|error|if let Some(rejection)=error.downcast_ref::<super::TaskRejection>() {rejection.rpc()} else if let Some(unknown)=error.downcast_ref::<super::unknown_outcome::UnknownOutcome>() {unknown.rpc()} else {RpcError::Failed(if error.to_string().contains("cloud_task_steer_selection_changed") {"This queued message uses different agent settings, or the active settings could not be verified. It is still queued. Stop the current turn and use Send now, or let it run afterward.".into()} else if error.to_string().contains("cannot save the selected agent") {error.to_string()} else {"The cloud action was not confirmed. Your draft is preserved; check the queue before retrying the same action.".into()})})
}

/// Read-only projection of a durable initial admission, visibly distinct from
/// the guest's transcript format/cursor and never an execution authority.
/// An API-registered empty conversation is a local presentation, never a guest checkpoint.
pub(super) fn registered_head(
    workspace: &Workspace,
    session_id: &str,
    session: &Value,
) -> Result<zuse_model::cloud::Head> {
    ensure!(
        session["sessionId"].as_str() == Some(session_id)
            && session["threadId"].as_str() == Some(workspace.chat_id.as_str()),
        "Cloud conversation scope changed."
    );
    let mut value = session
        .get("chat")
        .cloned()
        .unwrap_or_else(|| session.clone());
    value["id"] = json!(session_id);
    value["threadId"] = json!(workspace.chat_id);
    value["deviceId"] = json!(format!("cloud:{}", workspace.workspace_id));
    value["cwd"] = json!(".");
    value["archived"] = json!(false);
    ensure!(
        value["config"].is_object(),
        "Cloud conversation configuration is unavailable."
    );
    let chat: zuse_protocol::Chat = serde_json::from_value(value)?;
    let rows = Vec::new();
    Ok(zuse_model::cloud::Head {
        slack_results: Default::default(),
        schema_version: 1,
        payload_format: "zuse-cloud-admission-v1".into(),
        workspace_id: workspace.workspace_id.clone(),
        session_id: session_id.into(),
        runtime_generation: 0,
        cursor: zuse_model::cloud::Cursor {
            epoch: format!("admission-session:{session_id}"),
            version: 0,
        },
        chat: chat.clone(),
        conversations: vec![chat],
        transcript: zuse_model::TranscriptUpdate {
            frame: zuse_model::TranscriptFrame::Reset {
                reset: rows.clone().into(),
            },
            context_usage: None,
            replay_baseline: Some(zuse_model::TranscriptBaseline::capture(&rows)),
        },
        queue: json!([]),
        queue_paused: session["queuePaused"].as_bool().unwrap_or(false),
        older_message_sequence: None,
    })
}

pub(super) fn admission_head(
    workspace: &Workspace,
    intent: &Value,
    at: i64,
    queue_paused: bool,
) -> Result<zuse_model::cloud::Head> {
    let time = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at)
        .ok_or_else(|| anyhow::anyhow!("Invalid cloud admission timestamp."))?;
    let harness = if intent["agent"] == "claude" {
        json!("claude-code")
    } else {
        intent["agent"].clone()
    };
    let sandbox = match intent["runtimeMode"].as_str() {
        Some("full-access") => "danger-full-access",
        Some("auto") => "auto",
        _ => "workspace-write",
    };
    let chat: zuse_protocol::Chat = serde_json::from_value(
        json!({"id":workspace.initial_session_id,"deviceId":format!("cloud:{}",workspace.workspace_id),"title":intent["title"],"archived":workspace.archived(),"cwd":".","branch":workspace.branch,"checkoutId":null,"config":{"harness":harness,"model":intent["model"],"reasoning":intent["reasoning"],"modelOptions":intent.get("modelOptions").cloned().unwrap_or(json!({})),"sandbox":sandbox},"lastMessagePreview":intent["firstMessage"],"lastMessageAt":time,"createdAt":time,"threadId":workspace.chat_id}),
    )?;
    let assets: Vec<Asset> =
        serde_json::from_value(intent.get("attachments").cloned().unwrap_or(json!([])))?;
    let refs = assets.iter().map(asset_ref).collect::<Result<Vec<_>>>()?;
    let text = zuse_protocol::message_attachments::with_attachments(
        intent["firstMessage"].as_str().unwrap_or_default(),
        &refs,
    );
    let rows = vec![zuse_model::SessionMessageEntry {
        id: intent["turnId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .unwrap_or(&workspace.initial_session_id)
            .to_owned(),
        role: zuse_model::MessageRole::User,
        parts: vec![zuse_model::MessagePart::Text {
            id: "admission-text".into(),
            text,
        }],
        created_at: at,
        device_id: chat.device_id.clone(),
        status: None,
        continuation_of: None,
        duration_ms: None,
    }];
    Ok(zuse_model::cloud::Head {
        slack_results: Default::default(),
        schema_version: 1,
        payload_format: "zuse-cloud-admission-v1".into(),
        workspace_id: workspace.workspace_id.clone(),
        session_id: workspace.initial_session_id.clone(),
        runtime_generation: 0,
        cursor: zuse_model::cloud::Cursor {
            epoch: format!(
                "admission:{}",
                intent["commandId"]
                    .as_str()
                    .unwrap_or(&workspace.workspace_id)
            ),
            version: 0,
        },
        chat: chat.clone(),
        conversations: vec![chat],
        transcript: zuse_model::TranscriptUpdate {
            frame: zuse_model::TranscriptFrame::Reset {
                reset: rows.clone().into(),
            },
            context_usage: None,
            replay_baseline: Some(zuse_model::TranscriptBaseline::capture(&rows)),
        },
        queue: json!([]),
        queue_paused,
        older_message_sequence: None,
    })
}

#[cfg(test)]
mod tests;
