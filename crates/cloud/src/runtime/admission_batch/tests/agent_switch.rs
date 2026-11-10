//! The real admission path keeps provider changes on one task and preserves queued snapshots.
use super::*;
use zuse_protocol::{cloud_task_commands as w, session_commands::SessionCommandPayload};

pub(super) async fn exercise(
    control: &std::sync::Arc<RuntimeControl>,
    pool: &PgPool,
    access: &crate::access::Access,
    master: &crate::crypto::MasterKey,
    mut actual: Value,
    now: i64,
) {
    let id = "agent-switch";
    let sid = "session:agent-switch";
    let (raw, wrapped) = master.create_transcript_key(&access.owner_id, id).unwrap();
    actual["wrappedTranscriptKey"] = json!(wrapped);
    let config = &mut actual["requestConfig"];
    config["runtimeRpcProtocol"] = json!(zuse_protocol::cloud_gateway::RUST_RPC);
    config["rustCommandMailboxEnabled"] = json!(true);
    config["agent"] = json!("codex");
    config["agentConnectionId"] = json!("connection");
    config["agentConnectionOwnerId"] = json!(access.owner_id);
    config["rustActorId"] = json!(access.actor.account_id);
    let row = json!({"workspace_id":id,"account_id":access.owner_id,"chat_id":"chat:agent-switch","initial_session_id":sid,"wrapped_transcript_key":wrapped,"state":"provisioning","desired_state":"ready","request_config":config});
    let catalog = |agent: &str| json!({"id":if agent=="claude"{"claude-code"}else{agent},"name":"Fixture","defaultModel":"fixture","models":[{"id":"fixture","label":"Fixture"}]});
    let mut release = json!({"manifest":{"commandAdmission":{"managedAgentVersion":1}},"harnesses":[{"agent":"codex","catalog":catalog("codex")},{"agent":"claude","catalog":catalog("claude")},{"agent":"grok","catalog":catalog("grok")}]});
    release["harnesses"][2]["catalog"]["models"][0]["reasoningLevels"] = json!(["high"]);
    release["harnesses"][2]["catalog"]["models"][0]["options"] = json!([{"id":"speed","label":"Speed","defaultChoice":"fast","choices":[{"id":"fast","label":"Fast"}]}]);
    let intent = json!({"runtimeMode":"approval-required","model":"fixture","reasoning":null,"modelOptions":{},"firstMessage":"","title":"Agent switch fixture","attachments":[]});
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
    for agent in ["claude", "grok"] {
        sqlx::query("INSERT INTO cloud_agent_connections(connection_id,owner_id,idempotency_key,request_digest,agent,method,state,credential_ciphertext,created_at,updated_at) VALUES($1,$2,$1,'fixture',$3,'api-key','connected','synthetic-not-opened',$4,$4)")
            .bind(format!("switch-{agent}")).bind(&access.owner_id).bind(agent).bind(now).execute(pool).await.unwrap();
    }
    let key = zuse_cloud_runtime::mailbox::CommandKey::parse(&raw).unwrap();
    for (index, harness) in ["codex", "claude-code", "grok", "codex"]
        .into_iter()
        .enumerate()
    {
        let cid = format!("switch-command-{index}");
        let mid = format!("switch-message-{index}");
        let payload:SessionCommandPayload=serde_json::from_value(json!({"kind":"run","messageId":mid,"request":{"prompt":"Continue this conversation","harness":harness,"model":"fixture","reasoning":null,"modelOptions":{},"cwd":".","sandbox":"workspace-write","autoApprove":false,"resume":null,"worktree":null,"mcp":null,"attachments":[]}})).unwrap();
        let request = w::Request {
            command_id: cid.clone(),
            session_id: sid.into(),
            action: w::Action::Submit {
                message_id: mid,
                payload,
                attachments: vec![],
                based_on: None,
            },
            wake: w::WakePolicy::PreserveSleep,
        };
        let admitted = control
            .task_command_submit(id, access, request.clone(), now + index as i64)
            .await
            .unwrap();
        assert_eq!(
            control
                .task_command_submit(id, access, request, now + index as i64)
                .await
                .unwrap(),
            admitted,
            "reconnect never repeats the logical message"
        );
        let envelope:Value=sqlx::query_scalar("SELECT envelope FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(cid).fetch_one(pool).await.unwrap();
        let envelope = serde_json::from_value(envelope).unwrap();
        let command: zuse_protocol::cloud_commands::CloudSessionCommand =
            serde_json::from_slice(&key.decrypt(&envelope).unwrap()).unwrap();
        match command.payload {
            SessionCommandPayload::Run { request, .. } => {
                assert_eq!(String::from(request.harness.unwrap()), harness)
            }
            _ => panic!("expected a queued Run"),
        }
    }
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*)FROM zuse_rust_mailbox_commands WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        rows, 4,
        "all providers use the original task without rewriting queued commands"
    );
    let config: Value =
        sqlx::query_scalar("SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(config["agentConnectionId"], "connection");
    assert_eq!(
        config["agentConnectionBindings"]["claude"]["connectionId"],
        "switch-claude"
    );
    assert_eq!(
        config["agentConnectionBindings"]["grok"]["connectionId"],
        "switch-grok"
    );
    queued_selection(control, pool, access, &key, id, sid, now + 100).await;
}

async fn queued_selection(
    control: &std::sync::Arc<RuntimeControl>,
    pool: &PgPool,
    access: &crate::access::Access,
    key: &zuse_cloud_runtime::mailbox::CommandKey,
    id: &str,
    sid: &str,
    now: i64,
) {
    let req = |cid: &str, action: w::Action| w::Request {
        command_id: cid.into(),
        session_id: sid.into(),
        action,
        wake: w::WakePolicy::PreserveSleep,
    };
    let selected:w::RunSelection=serde_json::from_value(json!({"harness":"grok","model":"fixture","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write"})).unwrap();
    let input = |selection: Option<w::RunSelection>| w::Input {
        text: "Inspect the exact queued config".into(),
        run_selection: selection,
        attachments: vec![],
        file_refs: vec![],
        skill_refs: vec![],
        annotations: vec![],
        as_goal: false,
    };
    // Fixture supplies terminal facts, as the production progress matrix tests authenticated reports.
    sqlx::query(
        "UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE workspace_id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    control
        .task_command_submit(
            id,
            access,
            req(
                "selected-queue",
                w::Action::Queue {
                    queue_id: "selected-q".into(),
                    message_id: "selected-mid".into(),
                    input: input(Some(selected.clone())),
                    hold_for_turn_end: true,
                },
            ),
            now,
        )
        .await
        .unwrap();
    // A text-only edit must keep the immutable selection, not erase it.
    control
        .task_command_submit(
            id,
            access,
            req(
                "selected-edit",
                w::Action::Edit {
                    queue_id: "selected-q".into(),
                    revision: 1,
                    input: input(None),
                    lease_id: None,
                },
            ),
            now + 1,
        )
        .await
        .unwrap();
    let page = control.task_command_queue(id, access, sid).await.unwrap();
    let row = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "selected-q")
        .unwrap();
    assert_eq!(
        row["input"]["runSelection"],
        serde_json::to_value(&selected).unwrap()
    );
    // Change registered defaults after queuing; promotion must still use Grok.
    let later:SessionCommandPayload=serde_json::from_value(json!({"kind":"run","messageId":"later-config-mid","request":{"prompt":"Later Claude config","harness":"claude-code","model":"fixture","reasoning":null,"modelOptions":{},"cwd":".","sandbox":"workspace-write","autoApprove":false,"resume":null}})).unwrap();
    control
        .task_command_submit(
            id,
            access,
            req(
                "later-config",
                w::Action::Submit {
                    message_id: "later-config-mid".into(),
                    payload: later,
                    attachments: vec![],
                    based_on: None,
                },
            ),
            now + 2,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied' WHERE workspace_id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    for _ in 0..12 {
        control.work_task_drain_once(now + 100_000).await.unwrap();
        let claimed:bool=sqlx::query_scalar("SELECT state='claimed' FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id='selected-q'").bind(id).fetch_one(pool).await.unwrap();
        if claimed {
            break;
        }
    }
    let cid:String=sqlx::query_scalar("SELECT claimed_command_id FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id='selected-q'").bind(id).fetch_one(pool).await.unwrap();
    let envelope: Value = sqlx::query_scalar(
        "SELECT envelope FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND command_id=$2",
    )
    .bind(id)
    .bind(&cid)
    .fetch_one(pool)
    .await
    .unwrap();
    let command: zuse_protocol::cloud_commands::CloudSessionCommand = serde_json::from_slice(
        &key.decrypt(&serde_json::from_value(envelope).unwrap())
            .unwrap(),
    )
    .unwrap();
    let SessionCommandPayload::Run { request: run, .. } = command.payload else {
        panic!("queued selection must produce Run")
    };
    assert_eq!(run.harness, Some(selected.harness));
    assert_eq!(run.model.as_deref(), Some("fixture"));
    assert_eq!(run.reasoning, selected.reasoning);
    assert_eq!(run.model_options, selected.model_options);
    assert_eq!(run.sandbox, selected.sandbox);
    // Final admission revalidates actual catalog, permissions and bound authority.
    for (suffix, mut choice, expected) in [
        ("model", selected.clone(), "cloud_task_model_not_installed"),
        (
            "sandbox",
            selected.clone(),
            "cloud_task_permission_policy_changed",
        ),
        (
            "revoked",
            selected.clone(),
            "cloud_agent_connection_revoked",
        ),
    ] {
        if suffix == "model" {
            choice.model = "not-installed".into();
        }
        if suffix == "sandbox" {
            choice.sandbox = zuse_protocol::SandboxLevel::DangerFullAccess;
        }
        let q = format!("denied-{suffix}");
        control
            .task_command_submit(
                id,
                access,
                req(
                    &format!("queue-{q}"),
                    w::Action::Queue {
                        queue_id: q.clone(),
                        message_id: format!("mid-{q}"),
                        input: input(Some(choice)),
                        hold_for_turn_end: true,
                    },
                ),
                now + 3,
            )
            .await
            .unwrap();
        if suffix == "revoked" {
            sqlx::query("UPDATE cloud_agent_connections SET state='revoked' WHERE connection_id='switch-grok'").execute(pool).await.unwrap();
        }
        let denied = control
            .task_command_submit(
                id,
                access,
                req(
                    &format!("send-{q}"),
                    w::Action::SendNow {
                        queue_id: q.clone(),
                        revision: 1,
                    },
                ),
                now + 4,
            )
            .await;
        let error = match denied {
            Err(e) => e,
            Ok(_) => panic!("invalid queued config must remain queued"),
        };
        if suffix == "revoked" {
            assert!(
                error.code.contains("connection"),
                "unexpected revocation rejection {}",
                error.code
            );
        } else {
            assert_eq!(error.code, expected);
        }
        let state: String = sqlx::query_scalar(
            "SELECT state FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND queue_id=$2",
        )
        .bind(id)
        .bind(&q)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(state, "queued");
        control
            .task_command_submit(
                id,
                access,
                req(
                    &format!("delete-{q}"),
                    w::Action::Delete {
                        queue_id: q,
                        revision: 1,
                        lease_id: None,
                    },
                ),
                now + 5,
            )
            .await
            .unwrap();
        if suffix == "revoked" {
            sqlx::query("UPDATE cloud_agent_connections SET state='connected' WHERE connection_id='switch-grok'").execute(pool).await.unwrap();
        }
    }
    // Exact active evidence wins over later mutable registered defaults.
    sqlx::query(
        "UPDATE zuse_rust_task_command_execution SET state='completed' WHERE workspace_id=$1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE zuse_rust_task_command_execution SET state='running',turn_id='selected-active-turn' WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(&cid).execute(pool).await.unwrap();
    sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='applied',ever_leased=true WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(&cid).execute(pool).await.unwrap();
    let mut different = selected.clone();
    different.harness = zuse_protocol::HarnessId::ClaudeCode;
    different.reasoning = None;
    different.model_options.clear();
    control
        .task_command_submit(
            id,
            access,
            req(
                "steer-selected-q",
                w::Action::Queue {
                    queue_id: "steer-selected".into(),
                    message_id: "steer-selected-mid".into(),
                    input: input(Some(different)),
                    hold_for_turn_end: false,
                },
            ),
            now + 6,
        )
        .await
        .unwrap();
    let denied = control
        .task_command_submit(
            id,
            access,
            req(
                "steer-selection-different",
                w::Action::SteerNow {
                    queue_id: "steer-selected".into(),
                    revision: 1,
                    based_on: None,
                },
            ),
            now + 7,
        )
        .await;
    assert_eq!(
        denied.err().unwrap().code,
        "cloud_task_steer_selection_changed"
    );
    control
        .task_command_submit(
            id,
            access,
            req(
                "steer-selected-edit",
                w::Action::Edit {
                    queue_id: "steer-selected".into(),
                    revision: 1,
                    input: input(Some(selected.clone())),
                    lease_id: None,
                },
            ),
            now + 8,
        )
        .await
        .unwrap();
    control
        .task_command_submit(
            id,
            access,
            req(
                "steer-selection-same",
                w::Action::SteerNow {
                    queue_id: "steer-selected".into(),
                    revision: 2,
                    based_on: None,
                },
            ),
            now + 9,
        )
        .await
        .unwrap();
}
