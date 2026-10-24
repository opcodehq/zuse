//! Dashboard reads use the public RPC path and isolated on-disk usage exports.
use serde_json::json;
use std::sync::Arc;
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{
    HarnessId,
    usage::{Overview, SessionsPage},
};
use zuse_usage::import::{Format, Source};

#[tokio::test]
async fn usage_rpc_scopes_refreshes_and_paginates_without_starting_a_run() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("alpha");
    let other = temp.path().join("beta");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&other).unwrap();
    let exports = temp.path().join("exports");
    std::fs::create_dir(&exports).unwrap();
    let export = |id: &str, path: &std::path::Path, tokens: u64| json!({"sessionId":id,"timestamp":"2026-10-23T12:00:00Z","cwd":path,"message":{"id":id,"model":"claude-model","usage":{"input_tokens":tokens,"output_tokens":10}}});
    std::fs::write(
        exports.join("a.jsonl"),
        format!(
            "{}\n{}\n",
            export("alpha-chat", &project, 100),
            export("beta-chat", &other, 200)
        ),
    )
    .unwrap();
    let core = EngineCore::assemble(
        &temp.path().join("profile"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let sources = vec![Source::new(
        "claude",
        "Claude Code",
        Format::Claude,
        vec![exports.clone()],
    )];
    let usage =
        zuse_engine::usage::Usage::persistent(sources.clone(), temp.path().join("usage.sqlite"));
    let service = Arc::new(core.rpc_service().as_ref().clone().with_usage(usage));
    let client = zuse_rpc::memory_client(service);
    for (space_id, path) in [("alpha", &project), ("beta", &other)] {
        client.call(zuse_rpc::methods::MUTATE,json!({"op":"createSpace","spaceId":space_id,"deviceId":core.device_id,"path":path,"name":space_id})).await.unwrap();
    }
    let all: Overview = client
        .call_as(
            zuse_protocol::usage::OVERVIEW,
            json!({"since":"2026-10-20T00:00:00Z","timezone":"UTC"}),
        )
        .await
        .unwrap();
    assert_eq!(all.summary.tokens.total(), 320);
    assert_eq!(all.session_count, 2);
    let one: Overview = client
        .call_as(
            zuse_protocol::usage::OVERVIEW,
            json!({"projectId":"alpha","targetDeviceId":core.device_id}),
        )
        .await
        .unwrap();
    assert_eq!(one.summary.tokens.total(), 110);
    assert_eq!(one.session_count, 1);
    let first: SessionsPage = client
        .call_as(
            zuse_protocol::usage::SESSIONS,
            json!({"sort":"tokens","limit":1}),
        )
        .await
        .unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(first.rows[0].label, "beta-chat");
    assert_eq!(first.next_offset, Some(1));
    let filtered: SessionsPage = client
        .call_as(
            zuse_protocol::usage::SESSIONS,
            json!({"query":"ALPHA","providerId":"claude"}),
        )
        .await
        .unwrap();
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.rows[0].label, "alpha-chat");
    std::fs::write(
        exports.join("b.jsonl"),
        format!("{}\n", export("new-chat", &project, 300)),
    )
    .unwrap();
    let cached: Overview = client
        .call_as(zuse_protocol::usage::OVERVIEW, json!({}))
        .await
        .unwrap();
    assert_eq!(cached.session_count, 2);
    let refreshed: Overview = client
        .call_as(zuse_protocol::usage::OVERVIEW, json!({"forceRefresh":true}))
        .await
        .unwrap();
    assert_eq!(refreshed.session_count, 3);
    assert_eq!(refreshed.summary.tokens.total(), 630);
    assert!(
        client
            .call(
                zuse_protocol::usage::OVERVIEW,
                json!({"projectId":"not-a-project"})
            )
            .await
            .is_err()
    );
    assert!(
        client
            .call(
                zuse_protocol::usage::OVERVIEW,
                json!({"timezone":"not-a-timezone"})
            )
            .await
            .is_err()
    );
    assert!(
        client
            .call(
                zuse_protocol::usage::OVERVIEW,
                json!({"targetDeviceId":"another-computer"})
            )
            .await
            .is_err()
    );
    std::fs::remove_file(exports.join("a.jsonl")).unwrap();
    std::fs::remove_file(exports.join("b.jsonl")).unwrap();
    let reopened =
        zuse_engine::usage::Usage::persistent(sources.clone(), temp.path().join("usage.sqlite"));
    let reopened = zuse_rpc::memory_client(Arc::new(
        core.rpc_service().as_ref().clone().with_usage(reopened),
    ));
    let retained: Overview = reopened
        .call_as(zuse_protocol::usage::OVERVIEW, json!({}))
        .await
        .unwrap();
    assert_eq!(retained.summary.tokens.total(), 630);
    let separate = zuse_engine::usage::Usage::persistent(
        sources,
        temp.path().join("separate-profile-usage.sqlite"),
    );
    let separate = zuse_rpc::memory_client(Arc::new(
        core.rpc_service().as_ref().clone().with_usage(separate),
    ));
    let separate: Overview = separate
        .call_as(zuse_protocol::usage::OVERVIEW, json!({}))
        .await
        .unwrap();
    assert_eq!(separate.summary.record_count, 0);
    assert!(core.workspace.read_sessions().unwrap().is_empty());
    assert!(core.workspace.read_chats().unwrap().is_empty());
    core.shutdown().await;
}

#[test]
fn new_journal_timestamps_preserve_legacy_replay_and_make_native_usage_readable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.jsonl");
    let started = json!({"type":"sessionStarted","harness":"zuse","model":"native-model","tools":[],"cwd":"/project","sessionId":"native-session","assistantMessageId":"assistant"});
    std::fs::write(&path, format!("{}\n", json!({"seq":1,"event":started}))).unwrap();
    let journal = zuse_engine::RunJournal::open(temp.path()).unwrap();
    journal
        .append(
            "native",
            &zuse_protocol::AgentEvent::Usage {
                input_tokens: 40,
                output_tokens: 20,
            },
        )
        .unwrap();
    let replay = journal.replay("native", 0).unwrap();
    assert_eq!(replay.len(), 2);
    assert_eq!(replay[1].0, 2);
    let data = zuse_usage::import::Importer::default().scan(&[Source::new(
        "zuse",
        "Zuse (Beta)",
        Format::Native,
        vec![temp.path().into()],
    )]);
    assert_eq!(data.records.len(), 1);
    assert_eq!(data.records[0].tokens.total(), 60);
    assert!((chrono::Utc::now() - data.records[0].started_at).num_seconds() < 5);
}
