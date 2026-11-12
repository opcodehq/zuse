//! A computer whose guest runtime predates direct uploads or per-message
//! access, and cannot update in place, is refused those messages at
//! admission, before anything reaches its mailbox; plain messages keep
//! working on it.
use super::*;
use zuse_protocol::{cloud_task_commands as w, session_commands::SessionCommandPayload};

const FILE: &str = "/home/zuse/.zuse-data/uploads/a/shot.png";

pub(super) async fn exercise(
    control: &std::sync::Arc<RuntimeControl>,
    pool: &PgPool,
    access: &crate::access::Access,
    master: &crate::crypto::MasterKey,
    plan: impl Fn(&str) -> Value,
    now: i64,
) {
    let old = vec![
        json!("rust-rpc-v1"),
        json!("rust-session-commands-v1"),
        json!(w::CAPABILITY),
    ];
    let mut current = old.clone();
    current.push(json!(w::DIRECT_UPLOADS_CAPABILITY));
    current.push(json!(w::MESSAGE_ACCESS_MODE_CAPABILITY));
    // No stored capabilities at all is treated as an old runtime.
    for (id, capabilities) in [
        ("runtime-never-bootstrapped", None),
        ("runtime-old", Some(old)),
    ] {
        computer(control, pool, access, master, plan(id), capabilities, now).await;
        old_computer_refuses_files_and_access_changes(control, pool, access, id, now).await;
    }
    computer(
        control,
        pool,
        access,
        master,
        plan("runtime-current"),
        Some(current),
        now,
    )
    .await;
    current_computer_admits_files_and_access_changes(control, pool, access, "runtime-current", now)
        .await;
}

pub(super) async fn computer(
    control: &std::sync::Arc<RuntimeControl>,
    pool: &PgPool,
    access: &crate::access::Access,
    master: &crate::crypto::MasterKey,
    mut actual: Value,
    capabilities: Option<Vec<Value>>,
    now: i64,
) {
    let id = actual["workspaceId"].as_str().unwrap().to_owned();
    let (_, wrapped) = master.create_transcript_key(&access.owner_id, &id).unwrap();
    actual["wrappedTranscriptKey"] = json!(wrapped);
    let config = &mut actual["requestConfig"];
    config["runtimeRpcProtocol"] = json!(zuse_protocol::cloud_gateway::RUST_RPC);
    config["rustCommandMailboxEnabled"] = json!(true);
    config["agent"] = json!("codex");
    config["agentConnectionId"] = json!("connection");
    config["agentConnectionOwnerId"] = json!(access.owner_id);
    config["rustActorId"] = json!(access.actor.account_id);
    if let Some(capabilities) = capabilities {
        config["runtimeBootstrapReceipt"] =
            json!({"workspaceId":id,"generation":1,"gatewayEpoch":1,"capabilities":capabilities});
    }
    let row = json!({"workspace_id":id,"account_id":access.owner_id,"chat_id":format!("chat:{id}"),"initial_session_id":format!("session:{id}"),"wrapped_transcript_key":wrapped,"state":"provisioning","desired_state":"ready","request_config":config});
    let release = json!({"manifest":{"commandAdmission":{"managedAgentVersion":1}},"harnesses":[{"agent":"codex","catalog":{"id":"codex","name":"Fixture","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]}}]});
    let intent = json!({"runtimeMode":"approval-required","model":"fixture","reasoning":null,"modelOptions":{},"firstMessage":"","title":"Runtime version fixture","attachments":[]});
    actual["initialTaskCommand"] = serde_json::to_value(
        control
            .prepare_initial_task_command(&row, access, &intent, &release, now)
            .unwrap(),
    )
    .unwrap();
    sqlx::query_scalar::<_, Value>(WRITE_V2.as_str())
        .bind(actual)
        .fetch_one(pool)
        .await
        .unwrap();
}

pub(super) fn run(id: &str, cid: &str, files: &[&str], full_access: bool) -> w::Request {
    let mid = format!("{cid}-message");
    let (sandbox, auto) = if full_access {
        ("danger-full-access", true)
    } else {
        ("workspace-write", false)
    };
    let payload: SessionCommandPayload = serde_json::from_value(json!({"kind":"run","messageId":mid,"request":{"prompt":"Look at this","harness":"codex","model":"fixture","reasoning":null,"modelOptions":{},"cwd":".","sandbox":sandbox,"autoApprove":auto,"resume":null,"worktree":null,"mcp":null,"attachments":files}})).unwrap();
    w::Request {
        command_id: cid.into(),
        session_id: format!("session:{id}"),
        action: w::Action::Submit {
            message_id: mid,
            payload,
            attachments: vec![],
            based_on: None,
        },
        wake: w::WakePolicy::PreserveSleep,
    }
}

pub(super) async fn enqueued(pool: &PgPool, id: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM zuse_rust_mailbox_commands WHERE workspace_id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

pub(super) async fn admitted(pool: &PgPool, id: &str, cid: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_task_command_actions WHERE workspace_id=$1 AND command_id=$2)")
        .bind(id)
        .bind(cid)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn old_computer_refuses_files_and_access_changes(
    control: &std::sync::Arc<RuntimeControl>,
    pool: &PgPool,
    access: &crate::access::Access,
    id: &str,
    now: i64,
) {
    let before = enqueued(pool, id).await;
    for (cid, files, full_access) in [
        (format!("{id}-files"), vec![FILE], false),
        (format!("{id}-mode"), vec![], true),
        (format!("{id}-files-and-mode"), vec![FILE], true),
    ] {
        let refused = control
            .task_command_submit(id, access, run(id, &cid, &files, full_access), now)
            .await
            .unwrap_err();
        // Without in-place update support the app offers "Update computer".
        assert_eq!(
            refused.code,
            zuse_protocol::cloud_runtime_updates::RUNTIME_UPDATE_UNAVAILABLE,
            "{cid}"
        );
        assert_eq!(refused.status, axum::http::StatusCode::CONFLICT, "{cid}");
        assert!(!admitted(pool, id, &cid).await, "{cid} left no admission");
        assert_eq!(
            enqueued(pool, id).await,
            before,
            "{cid} reached the mailbox"
        );
    }
    // The refused access change did not stick: the plain message still runs
    // with the computer's bound mode and is delivered as before.
    let plain = format!("{id}-plain");
    control
        .task_command_submit(id, access, run(id, &plain, &[], false), now + 1)
        .await
        .unwrap();
    assert!(admitted(pool, id, &plain).await);
    assert_eq!(enqueued(pool, id).await, before + 1);
}

async fn current_computer_admits_files_and_access_changes(
    control: &std::sync::Arc<RuntimeControl>,
    pool: &PgPool,
    access: &crate::access::Access,
    id: &str,
    now: i64,
) {
    let before = enqueued(pool, id).await;
    for (index, (cid, files, full_access)) in [
        (format!("{id}-files"), vec![FILE], false),
        (format!("{id}-mode"), vec![], true),
        (format!("{id}-files-back-to-default"), vec![FILE], false),
    ]
    .into_iter()
    .enumerate()
    {
        control
            .task_command_submit(
                id,
                access,
                run(id, &cid, &files, full_access),
                now + index as i64,
            )
            .await
            .unwrap();
        assert!(admitted(pool, id, &cid).await, "{cid}");
        assert_eq!(enqueued(pool, id).await, before + 1 + index as i64, "{cid}");
    }
}
