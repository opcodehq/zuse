use super::*;
fn asset() -> Asset {
    Asset {
        asset_id: "asset-a".into(),
        mime_type: "text/plain".into(),
        original_name: "notes.txt".into(),
        size_bytes: 5,
        sha256: "fixture-digest".into(),
    }
}
fn run(cwd: &str, files: Vec<String>) -> Value {
    json!({"chatId":"session-a","cloudCommandId":"action-a","command":{"kind":"run","messageId":"user-a","request":{"prompt":zuse_protocol::message_attachments::with_attachments("inspect",&files),"harness":"codex","model":"model-a","reasoning":null,"modelOptions":{},"cwd":cwd,"sandbox":"workspace-write","autoApprove":false,"resume":null,"attachments":files,"worktree":null,"mcp":null}}})
}
#[test]
fn ordinary_run_has_stable_ids_and_separate_verified_assets() {
    let reference = asset_ref(&asset()).unwrap();
    let result = normalize(methods::QUEUE_COMMAND, &run(".", vec![reference])).unwrap();
    assert_eq!(result.command_id, "action-a");
    assert_eq!(result.session_id, "session-a");
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["wake"], "preserve-sleep");
    assert_eq!(value["action"]["messageId"], "user-a");
    assert_eq!(value["action"]["payload"]["request"]["prompt"], "inspect");
    assert!(
        value["action"]["payload"]["request"]
            .get("attachments")
            .is_none()
    );
    assert_eq!(value["action"]["attachments"][0]["assetId"], "asset-a");
}
#[test]
fn unresolved_machine_paths_and_unreviewed_queue_revisions_fail_closed() {
    assert!(
        normalize(
            methods::QUEUE_COMMAND,
            &run(".", vec!["/Users/test/secret.txt".into()])
        )
        .is_err()
    );
    assert!(normalize(methods::QUEUE_COMMAND, &run("/Users/test/repo", vec![])).is_err());
    let relative = serde_json::to_value(
        normalize(methods::QUEUE_COMMAND, &run("packages/api", vec![])).unwrap(),
    )
    .unwrap();
    assert_eq!(
        relative["action"]["payload"]["request"]["cwd"],
        "packages/api"
    );

    let mut request = run(".", vec![]);
    request["command"]["request"]["mcp"] = json!({"command":"/Users/test/tool","args":[]});
    assert!(normalize(methods::QUEUE_COMMAND, &request).is_err());
    assert!(
        normalize(
            methods::SEND_QUEUED_MESSAGE_NOW,
            &json!({"cloudCommandId":"action-a","chatId":"session-a","id":"queued-a"})
        )
        .is_err()
    );
    let result=normalize(methods::MOVE_QUEUED_MESSAGE,&json!({"cloudCommandId":"action-a","chatId":"session-a","id":"queued-a","revision":7,"toIndex":2})).unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["action"],
        json!({"type":"move","queueId":"queued-a","revision":7,"toIndex":2})
    );
}
#[test]
fn queued_identity_survives_promotion_and_assets_roundtrip_without_local_paths() {
    let reference = asset_ref(&asset()).unwrap();
    assert!(parse_asset_ref(&reference).unwrap() == asset());
    let request=normalize(methods::QUEUE_MESSAGE,&json!({"cloudCommandId":"action-a","chatId":"session-a","text":"later","attachments":[reference],"holdForTurnEnd":true})).unwrap();
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["action"]["queueId"], value["action"]["messageId"]);
    assert_eq!(value["action"]["holdForTurnEnd"], true);
    let page=queue_shape(json!({"items":[{"id":"q","text":"later","revision":8,"attachments":[asset()],"editing":{"actorId":"someone","expiresAt":12}}]})).unwrap();
    assert!(
        page["items"][0]["attachments"][0]
            .as_str()
            .unwrap()
            .starts_with(ASSET_PREFIX)
    );
    assert_eq!(page["items"][0]["deliveryGate"]["kind"], "editing");
    assert_eq!(page["items"][0]["revision"], 8);
}
#[test]
fn lease_receipt_binds_reviewed_revision_text_and_exact_asset_refs() {
    let page = queue_shape(
        json!({"items":[{"id":"q","text":"original","revision":3,"attachments":[asset()]}]}),
    )
    .unwrap();
    let value = adapt(
        methods::BEGIN_QUEUED_MESSAGE_EDIT,
        &json!({"id":"q","revision":3}),
        json!({"id":"q","revision":3,"leaseId":"lease-a","expiresAt":9000}),
        Some(&page),
    )
    .unwrap();
    assert_eq!(value["text"], "original");
    assert_eq!(
        value["baseTextHash"],
        format!("{:x}", Sha256::digest(b"original"))
    );
    assert!(zuse_protocol::presentation::queue_edit_lease(&value).is_some());
    assert!(
        adapt(
            methods::BEGIN_QUEUED_MESSAGE_EDIT,
            &json!({"id":"q","revision":2}),
            json!({"leaseId":"lease-a","expiresAt":9000}),
            Some(&page)
        )
        .is_err()
    );
}
#[test]
fn admission_projection_is_not_a_runtime_checkpoint_and_uses_canonical_user_mid() {
    let row:Workspace=serde_json::from_value(json!({"workspaceId":"workspace-a","chatId":"thread-a","initialSessionId":"session-a","branch":"task/a","state":"provisioning","desiredState":"ready","statusCode":"allocating","revision":1})).unwrap();
    let head=admission_head(&row,&json!({"commandId":"launch:workspace-a","turnId":"turn:workspace-a","agent":"codex","model":"model-a","title":"task","runtimeMode":"approval-required","firstMessage":"inspect"}),1000,true).unwrap();
    assert_eq!(head.payload_format, "zuse-cloud-admission-v1");
    assert!(head.queue_paused);
    assert_eq!(head.runtime_generation, 0);
    let zuse_model::TranscriptFrame::Reset { reset } = head.transcript.frame else {
        panic!("admission must be a complete read-only projection")
    };
    assert_eq!(reset[0].id, "turn:workspace-a");
    assert_eq!(head.chat.thread_id.as_deref(), Some("thread-a"));
}
#[test]
fn empty_registered_session_is_bound_to_task_and_keeps_config_without_guest_cursor() {
    let row:Workspace=serde_json::from_value(json!({"workspaceId":"workspace-a","chatId":"thread-a","initialSessionId":"initial","branch":"task/a","state":"paused","desiredState":"paused","statusCode":"paused","revision":2})).unwrap();
    let session = json!({"sessionId":"followup","threadId":"thread-a","createdAt":"2026-10-30T00:00:00Z","queuePaused":true,"title":"Follow-up","config":{"harness":"codex","model":"model-a","sandbox":"workspace-write"}});
    let head = registered_head(&row, "followup", &session).unwrap();
    assert_eq!(head.session_id, "followup");
    assert!(head.queue_paused);
    assert_eq!(head.chat.thread_id.as_deref(), Some("thread-a"));
    assert_eq!(head.runtime_generation, 0);
    assert_eq!(head.cursor.version, 0);
    assert_eq!(head.payload_format, "zuse-cloud-admission-v1");
    assert!(
        matches!(head.transcript.frame,zuse_model::TranscriptFrame::Reset{reset} if reset.is_empty())
    );
    let mut wrong = session;
    wrong["threadId"] = json!("other-thread");
    assert!(registered_head(&row, "followup", &wrong).is_err());
}
async fn sleeping_task_http_admission(wake: Option<wire::WakePolicy>) -> Value {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use futures::StreamExt;
    let admitted = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let capture = admitted.clone();
    let app=Router::new().route("/v1/cloud/workspaces/workspace-a",get(||async{Json(json!({"workspaceId":"workspace-a","state":"paused","commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2}))})).route("/v1/cloud/workspaces/workspace-a/task-commands",post(move|Json(body):Json<Value>|{let capture=capture.clone();async move{capture.lock().unwrap().push(body.clone());Json(json!({"commandId":body["commandId"],"durablyAdmitted":true,"state":"waiting-for-runtime"}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let mut config = crate::auth::AuthConfig::new(&origin, root.path());
    config.workos_client_id = None;
    let auth = Auth::new(config);
    let account = auth.user_id().unwrap();
    let api = Arc::new(CloudApi::new(&origin).unwrap());
    assert!(api.environments.get(&account, "workspace-a").is_none());
    let mut params = run(".", vec![]);
    if let Some(wake) = wake {
        params["wake"] = json!(wake);
    }
    let reply = maybe_forward(
        api.clone(),
        auth,
        account.clone(),
        None,
        "workspace-a".into(),
        methods::QUEUE_COMMAND,
        params,
        17,
    )
    .await
    .unwrap()
    .unwrap();
    let zuse_rpc::RpcReply::Stream(mut stream) = reply else {
        panic!("forward reply must preserve RPC envelope")
    };
    assert_eq!(stream.next().await.unwrap()["ok"]["durablyAdmitted"], true);
    assert!(api.environments.get(&account, "workspace-a").is_none());
    assert_eq!(admitted.lock().unwrap().len(), 1);
    let recorded = admitted.lock().unwrap()[0].clone();
    server.abort();
    recorded
}
#[tokio::test]
async fn sleeping_task_admits_via_http_without_execution_connection_or_resume() {
    assert_eq!(
        sleeping_task_http_admission(None).await["wake"],
        "preserve-sleep"
    );
}
#[tokio::test]
async fn foreground_user_send_carries_explicit_wake_in_the_same_http_admission_without_guest_connection()
 {
    assert_eq!(
        sleeping_task_http_admission(Some(wire::WakePolicy::ExplicitResume)).await["wake"],
        "explicit-resume"
    );
}

#[test]
fn queue_normalization_retains_explicit_selection_and_legacy_input_omits_it() {
    let selection = json!({"harness":"grok","model":"selected-grok","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write"});
    let params = json!({"chatId":"session-a","cloudCommandId":"selected-queue","text":"later","runSelection":selection});
    let value = serde_json::to_value(normalize(methods::QUEUE_MESSAGE, &params).unwrap()).unwrap();
    assert_eq!(value["action"]["input"]["runSelection"], selection);
    let legacy = serde_json::to_value(
        normalize(
            methods::QUEUE_MESSAGE,
            &json!({"chatId":"session-a","cloudCommandId":"legacy-queue","text":"later"}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(legacy["action"]["input"].get("runSelection").is_none());
}

#[test]
fn only_explicit_foreground_runs_request_wake_and_background_actions_preserve_sleep() {
    let mut foreground = run(".", vec![]);
    foreground["wake"] = json!("explicit-resume");
    let selected = normalize(methods::QUEUE_COMMAND, &foreground).unwrap();
    assert_eq!(
        serde_json::to_value(selected).unwrap()["wake"],
        "explicit-resume"
    );
    foreground["wake"] = Value::Null;
    assert_eq!(
        serde_json::to_value(normalize(methods::QUEUE_COMMAND, &foreground).unwrap()).unwrap()["wake"],
        "preserve-sleep"
    );
    foreground["wake"] = json!("invalid-wake");
    assert!(normalize(methods::QUEUE_COMMAND, &foreground).is_err());
    for (method, extra) in [
        (methods::QUEUE_MESSAGE, json!({"text":"background"})),
        (
            methods::UPDATE_QUEUED_MESSAGE,
            json!({"id":"queued-row","revision":1,"text":"edited"}),
        ),
        (
            methods::CREATE_THREAD_SESSION,
            json!({"title":"Empty conversation"}),
        ),
    ] {
        let mut params = json!({"chatId":"session-a","cloudCommandId":"background-command","wake":"explicit-resume"});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            serde_json::to_value(normalize(method, &params).unwrap()).unwrap()["wake"],
            "preserve-sleep"
        );
    }
}

#[test]
fn passive_attachment_requires_confirmed_explicit_run_and_current_actor_scope() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = crate::auth::AuthConfig::new("http://127.0.0.1:9", directory.path());
    config.workos_client_id = None;
    let auth = Auth::new(config);
    let account = auth.user_id().unwrap();
    let mut params = run(".", vec![]);
    params["wake"] = json!("explicit-resume");
    let request = normalize(methods::QUEUE_COMMAND, &params).unwrap();
    let receipt = json!({"durablyAdmitted":true,"commandId":request.command_id});
    let hint = passive_attachment(&auth, &account, None, "workspace", &request, &receipt)
        .unwrap()
        .unwrap();
    assert_eq!(hint["mode"], "passive");
    assert_eq!(hint["accountId"], account);
    assert!(passive_attachment(&auth, "other", None, "workspace", &request, &receipt).is_err());
    assert!(
        passive_attachment(
            &auth,
            &account,
            Some("other-org"),
            "workspace",
            &request,
            &receipt
        )
        .is_err()
    );
    for receipt in [
        json!({"durablyAdmitted":false,"commandId":request.command_id}),
        json!({"durablyAdmitted":true,"commandId":"different"}),
    ] {
        assert!(
            passive_attachment(&auth, &account, None, "workspace", &request, &receipt)
                .unwrap()
                .is_none()
        );
    }
    let mut background = request.clone();
    background.wake = wire::WakePolicy::PreserveSleep;
    assert!(
        passive_attachment(&auth, &account, None, "workspace", &background, &receipt)
            .unwrap()
            .is_none()
    );
    let mut control = request.clone();
    if let wire::Action::Submit { payload, .. } = &mut control.action {
        *payload = SessionCommandPayload::Interrupt {};
    }
    assert!(
        passive_attachment(&auth, &account, None, "workspace", &control, &receipt)
            .unwrap()
            .is_none()
    );
    for (method, extra) in [
        (methods::QUEUE_MESSAGE, json!({"text":"later"})),
        (
            methods::UPDATE_QUEUED_MESSAGE,
            json!({"id":"row","revision":1,"text":"edit"}),
        ),
        (methods::CREATE_THREAD_SESSION, json!({"title":"empty"})),
    ] {
        let mut p =
            json!({"chatId":"session-a","cloudCommandId":"action-a","wake":"explicit-resume"});
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        let mut background = normalize(method, &p).unwrap();
        background.wake = wire::WakePolicy::ExplicitResume;
        assert!(
            passive_attachment(&auth, &account, None, "workspace", &background, &receipt)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn a_queued_message_keeps_files_on_the_computer_apart_from_server_assets() {
    let computer = "/home/zuse/.zuse-data/uploads/u1/shot.png";
    let reference = asset_ref(&asset()).unwrap();
    let params = json!({"chatId":"session-a","cloudCommandId":"queued-files","text":"later",
        "attachments":[reference, computer]});
    let value = serde_json::to_value(normalize(methods::QUEUE_MESSAGE, &params).unwrap()).unwrap();
    let input = &value["action"]["input"];
    assert_eq!(input["computerFiles"], json!([computer]));
    assert_eq!(input["attachments"][0]["assetId"], "asset-a");
    assert_eq!(input["attachments"].as_array().unwrap().len(), 1);
    // A row read back and edited keeps its computer files exactly once.
    let edited = json!({"chatId":"session-a","cloudCommandId":"edit","id":"row","revision":1,
        "text":"edited","attachments":[computer],"computerFiles":[computer]});
    let value =
        serde_json::to_value(normalize(methods::UPDATE_QUEUED_MESSAGE, &edited).unwrap()).unwrap();
    assert_eq!(value["action"]["input"]["computerFiles"], json!([computer]));
    assert!(
        value["action"]["input"]["attachments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // A relative path is neither kind of file.
    let bad =
        json!({"chatId":"session-a","cloudCommandId":"bad","text":"x","attachments":["shot.png"]});
    assert!(normalize(methods::QUEUE_MESSAGE, &bad).is_err());
    let many = json!({"chatId":"session-a","cloudCommandId":"many","text":"x",
        "attachments":(0..9).map(|n|format!("/u/{n}.png")).collect::<Vec<_>>()});
    assert!(normalize(methods::QUEUE_MESSAGE, &many).is_err());
}

#[test]
fn queue_rows_list_files_on_the_computer_by_their_path_there() {
    let page = queue_shape(json!({"items":[{"id":"row","attachments":[asset()],
        "computerFiles":["/home/zuse/.zuse-data/uploads/u1/shot.png"],"createdAt":5}]}))
    .unwrap();
    let files = page["items"][0]["attachments"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[0].as_str().unwrap().starts_with(ASSET_PREFIX));
    assert_eq!(files[1], "/home/zuse/.zuse-data/uploads/u1/shot.png");
}

#[test]
fn computer_files_upload_to_and_read_from_the_computer_not_the_server() {
    assert!(computer_file_call(
        methods::UPLOAD_CHUNK,
        &json!({"uploadId":"u","computer":true})
    ));
    assert!(computer_file_call(
        methods::UPLOAD_COMMIT,
        &json!({"uploadId":"u","computer":true})
    ));
    assert!(!computer_file_call(
        methods::UPLOAD_CHUNK,
        &json!({"uploadId":"u"})
    ));
    assert!(computer_file_call(
        methods::READ_ATTACHMENT_CHUNK,
        &json!({"path":"/home/zuse/.zuse-data/uploads/u1/shot.png"})
    ));
    let reference = asset_ref(&asset()).unwrap();
    assert!(!computer_file_call(
        methods::READ_ATTACHMENT_CHUNK,
        &json!({ "path": reference })
    ));
    assert!(!computer_file_call(
        methods::QUEUE_MESSAGE,
        &json!({"computer":true})
    ));
}
