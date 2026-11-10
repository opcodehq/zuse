//! Actual HTTP admission + retired Manager + fresh gateway subscription.
use super::*;
use axum::{
    Json, Router,
    extract::ws::{Message, WebSocketUpgrade},
    routing::{get, post},
};
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn admitted_foreground_run_reattaches_retired_environment_passively_and_streams_history() {
    for direct in [false, true] {
        let paused = Arc::new(AtomicBool::new(true));
        let admissions = Arc::new(AtomicUsize::new(0));
        let resumes = Arc::new(AtomicUsize::new(0));
        let trace = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let socket_url = format!(
            "ws://{}/v1/cloud/workspaces/foreground/gateway",
            listener.local_addr().unwrap()
        );
        let row_paused = paused.clone();
        let pending_reads = Arc::new(AtomicUsize::new(0));
        let row_trace = trace.clone();
        let command_paused = paused.clone();
        let command_count = admissions.clone();
        let command_trace = trace.clone();
        let resume_count = resumes.clone();
        let ticket_trace = trace.clone();
        let app=Router::new()
            .route("/v1/cloud/workspaces/foreground", get(move || {
                let paused=row_paused.clone();let trace=row_trace.clone();let pending_reads=pending_reads.clone();async move {
                    trace.lock().unwrap().push("metadata");
                    let sleeping=paused.load(Ordering::SeqCst);
                    let starting=!sleeping&&pending_reads.fetch_add(1,Ordering::SeqCst)==0;
                    Json(json!({"workspaceId":"foreground","chatId":"thread","initialSessionId":"session","branch":"task/retained","state":if sleeping{"paused"}else if starting{"provisioning"}else{"ready"},"desiredState":if sleeping{"paused"}else{"ready"},"statusCode":if sleeping{"paused"}else{"ready"},"revision":3,"rpcProtocol":"zuse-rpc-v1","commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2}))
                }
            }))
            .route("/v1/cloud/workspaces/foreground/task-commands", post(move |headers:axum::http::HeaderMap,Json(body):Json<Value>| {
                let paused=command_paused.clone();let count=command_count.clone();let trace=command_trace.clone();async move {
                    assert!(headers.get("authorization").is_some());
                    assert!(headers.get("x-zuse-workspace").is_none());
                    assert_eq!(body["wake"],"explicit-resume");
                    assert_eq!(body["action"]["payload"]["kind"],"run");
                    count.fetch_add(1,Ordering::SeqCst);trace.lock().unwrap().push("admitted");
                    // The one admission owns server wake; attachment must not
                    // send a second independent Resume mutation.
                    paused.store(false,Ordering::SeqCst);
                    Json(json!({"commandId":body["commandId"],"durablyAdmitted":true,"state":"waiting-for-runtime"}))
                }
            }))
            .route("/v1/cloud/workspaces/foreground/resume",post(move || {
                let count=resume_count.clone();async move {count.fetch_add(1,Ordering::SeqCst);Json(json!({}))}
            }))
            .route("/v1/cloud/workspaces/foreground/gateway/ticket",post(move || {
                let url=socket_url.clone();let trace=ticket_trace.clone();async move {
                    trace.lock().unwrap().push("ticket");
                    Json(json!({"workspaceId":"foreground","wsUrl":url,"protocol":zuse_protocol::cloud_gateway::RUST_PROTOCOL,"rpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"credential":"local-test-grant","generation":1,"gatewayEpoch":1,"expiresAt":chrono::Utc::now().timestamp_millis()+60000}))
                }
            }))
            .route("/v1/cloud/workspaces/foreground/gateway",get(|ws:WebSocketUpgrade|async move {
                ws.protocols([zuse_protocol::cloud_gateway::RUST_PROTOCOL]).on_upgrade(|mut socket|async move {
                    socket.send(Message::Text(json!({"_zuseGateway":"availability","state":"available"}).to_string().into())).await.unwrap();
                    while let Some(Ok(Message::Text(text)))=socket.next().await {
                        let request:Value=serde_json::from_str(&text).unwrap();
                        if request["cancel"]==true {continue;}
                        assert_eq!(request["method"],zuse_protocol::methods::WATCH_DOC_MESSAGES);
                        assert_eq!(request["params"]["chatId"],"session");
                        let entry=|id:&str,text:&str|json!({"id":id,"role":"assistant","parts":[{"id":format!("part-{id}"),"kind":"text","text":text}],"createdAt":1,"deviceId":"runtime"});
                        socket.send(Message::Text(json!({"id":request["id"],"item":{"reset":[entry("retained","Retained answer"),entry("fresh","Actual fresh stream")]}}).to_string().into())).await.unwrap();
                    }
                })
            }));
        let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let directory = tempfile::tempdir().unwrap();
        let mut config = crate::auth::AuthConfig::new(&origin, directory.path());
        config.workos_client_id = None;
        let auth = Auth::new(config);
        let account = auth.user_id().unwrap();
        let api = Arc::new(CloudApi::new(&origin).unwrap());
        let old = api.environments.open(
            api.clone(),
            auth.clone(),
            account.clone(),
            "foreground".into(),
            false,
        );
        let mut old_updates = old.updates();
        tokio::time::timeout(Duration::from_secs(3), async {
            while old_updates.next().await.unwrap()["state"] != "paused" {}
        })
        .await
        .unwrap();
        api.environments.retire_scoped(&account, None, "foreground");
        assert!(old.cancel.is_cancelled());
        assert!(api.environments.get(&account, "foreground").is_none());
        let params = json!({"chatId":"session","cloudCommandId":"foreground-once","wake":"explicit-resume","command":{"kind":"run","messageId":"foreground-message","request":{"prompt":"continue","harness":"codex","model":"local-test-model","reasoning":null,"modelOptions":{},"cwd":".","sandbox":"workspace-write","autoApprove":false,"resume":null,"attachments":[],"worktree":null,"mcp":null}}});
        let receipt = if direct {
            let request = super::super::task_commands::normalize(
                zuse_protocol::methods::QUEUE_COMMAND,
                &params,
            )
            .unwrap();
            api.handle(&auth,zuse_protocol::methods::CLOUD_TASK_COMMAND,json!({"workspaceId":"foreground","expectedAccountId":account,"expectedOrganizationId":null,"request":request})).await.unwrap()["data"].clone()
        } else {
            let RpcReply::Stream(mut stream) = super::super::task_commands::maybe_forward(
                api.clone(),
                auth.clone(),
                account.clone(),
                None,
                "foreground".into(),
                zuse_protocol::methods::QUEUE_COMMAND,
                params,
                13,
            )
            .await
            .unwrap()
            .unwrap() else {
                panic!("stream reply required")
            };
            stream.next().await.unwrap()["ok"].clone()
        };
        assert_eq!(receipt["commandId"], "foreground-once");
        assert_eq!(receipt["durablyAdmitted"], true);
        let hint = &receipt["passiveAttachment"];
        assert_eq!(
            hint["mode"], "passive",
            "durable foreground receipt must signal passive attachment"
        );
        assert_eq!(hint["accountId"], account);
        assert!(hint["organizationId"].is_null());
        assert_eq!(hint["workspaceId"], "foreground");
        assert_eq!(hint["sessionId"], "session");
        assert_eq!(hint["commandId"], "foreground-once");
        // This subscription, owned by the UI, must keep the new environment
        // alive. Opening a weak entry and dropping it would lose the socket.
        let fresh = api.environments.open(
            api.clone(),
            auth.clone(),
            account.clone(),
            "foreground".into(),
            false,
        );
        assert!(!Arc::ptr_eq(&old, &fresh));
        let mut updates = fresh.updates();
        tokio::time::timeout(Duration::from_secs(3), async {
            while updates.next().await.unwrap()["state"] != "ready" {}
        })
        .await
        .unwrap();
        let mut messages = fresh
            .ready_client()
            .unwrap()
            .forward(
                zuse_protocol::methods::WATCH_DOC_MESSAGES,
                json!({"chatId":"session"}),
            )
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(3), messages.recv())
            .await
            .unwrap()
            .unwrap();
        let frame = serde_json::to_value(frame).unwrap();
        let transcript: zuse_model::TranscriptFrame =
            serde_json::from_value(frame["item"].clone()).unwrap();
        let zuse_model::TranscriptFrame::Reset { reset } = transcript else {
            panic!("canonical reset expected")
        };
        assert_eq!(
            reset
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["retained", "fresh"]
        );
        assert_eq!(admissions.load(Ordering::SeqCst), 1);
        assert_eq!(resumes.load(Ordering::SeqCst), 0);
        let trace = trace.lock().unwrap();
        assert!(
            trace.iter().position(|v| *v == "admitted").unwrap()
                < trace.iter().position(|v| *v == "ticket").unwrap()
        );
    }
}
