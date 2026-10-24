use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::{fs, path::Path};
use zuse_usage::{
    CostStatus, Request, SessionSort,
    aggregate::Selection,
    import::{Format, Importer, Source},
};
fn time(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
fn lines(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        rows.iter().map(|v| format!("{v}\n")).collect::<String>() + "{torn tail",
    )
    .unwrap();
}
fn claude(id: &str, at: &str, out: u64, cost: f64) -> Value {
    json!({"timestamp":at,"sessionId":"claude-alpha","cwd":"/projects/alpha","requestId":"request","costUSD":cost,"message":{"id":id,"model":"claude-model","usage":{"input_tokens":100,"output_tokens":out,"cache_read_input_tokens":30,"cache_creation_input_tokens":10}}})
}
#[test]
fn imports_real_exports_deduplicates_updates_and_projects_the_dashboard() {
    let temp = tempfile::tempdir().unwrap();
    let claude_dir = temp.path().join("claude");
    let codex_dir = temp.path().join("codex");
    lines(
        &claude_dir.join("a.jsonl"),
        &[
            claude("m1", "2026-10-23T01:00:00Z", 5, 0.5),
            claude("m1", "2026-10-23T01:00:00Z", 20, 0.6),
            claude("prior", "2026-10-17T12:00:00Z", 5, 0.1),
        ],
    );
    let usage = |input, cached, output, reasoning, total| json!({"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output,"reasoning_output_tokens":reasoning,"total_tokens":total});
    let codex = vec![
        json!({"type":"session_meta","payload":{"cwd":"/projects/beta"}}),
        json!({"type":"turn_context","payload":{"model":"codex-model"}}),
        json!({"type":"event_msg","timestamp":"2026-10-23T10:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":usage(100,20,20,5,125)}}}),
        json!({"type":"event_msg","timestamp":"2026-10-23T11:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":usage(200,50,50,10,260)}}}),
    ];
    lines(&codex_dir.join("rollout.jsonl"), &codex);
    lines(&codex_dir.join("copy.jsonl"), &codex);
    let sources = vec![
        Source::new(
            "claude",
            "Claude Code",
            Format::Claude,
            vec![claude_dir.clone()],
        ),
        Source::new("codex", "Codex", Format::Codex, vec![codex_dir.clone()]),
    ];
    let mut importer = Importer::default();
    let data = importer.scan(&sources);
    assert_eq!(data.records.len(), 4);
    let selection = Selection {
        since: Some(time("2026-10-18T00:00:00Z")),
        until: Some(time("2026-10-24T00:00:00Z")),
        timezone: Some("UTC".into()),
        ..Default::default()
    };
    let report = data
        .overview(&selection, time("2026-10-24T00:00:00Z"))
        .unwrap();
    assert_eq!(report.summary.tokens.total(), 420);
    assert_eq!(report.summary.cost_status, CostStatus::Partial);
    assert_eq!(report.summary.cost_usd, Some(0.6));
    assert_eq!(report.session_count, 2);
    assert_eq!(report.by_source.len(), 2);
    assert_eq!(report.by_model.len(), 2);
    assert_eq!(report.by_project.len(), 2);
    assert_eq!(
        report.previous_summary.as_ref().unwrap().tokens.total(),
        145
    );
    let mut project = selection.clone();
    project.project_paths = Some(vec!["/projects/alpha".into()]);
    let one = data.overview(&project, Utc::now()).unwrap();
    assert_eq!(one.summary.tokens.total(), 160);
    assert_eq!(one.session_count, 1);
    project.project_paths = Some(vec!["/projects/alph".into()]);
    assert_eq!(
        data.overview(&project, Utc::now())
            .unwrap()
            .summary
            .record_count,
        0,
        "Similar project names must not match a path prefix"
    );
    project.project_paths = Some(vec![]);
    assert_eq!(
        data.overview(&project, Utc::now()).unwrap().session_count,
        0,
        "An unresolved project must not silently expand to global usage"
    );
    let mut request = Request {
        limit: Some(1),
        ..Default::default()
    };
    let page = data.sessions(&selection, &request).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.rows[0].summary.tokens.total(), 260);
    assert_eq!(page.next_offset, Some(1));
    request.offset = 1;
    let next = data.sessions(&selection, &request).unwrap();
    assert_eq!(next.rows[0].key, "claude-alpha");
    assert_eq!(next.next_offset, None);
    request.offset = 0;
    request.provider_id = Some("claude".into());
    request.query = "ALPHA".into();
    request.sort = SessionSort::Cost;
    assert_eq!(data.sessions(&selection, &request).unwrap().total, 1);
    let mut local = selection.clone();
    local.timezone = Some("America/Los_Angeles".into());
    let local = data.overview(&local, Utc::now()).unwrap();
    assert_eq!(local.groups[0].key, "2026-10-22");
    let saved = serde_json::to_value(
        data.overview(&selection, time("2026-10-24T00:00:00Z"))
            .unwrap(),
    )
    .unwrap();
    let cached = importer.scan(&sources);
    assert_eq!(
        serde_json::to_value(
            cached
                .overview(&selection, time("2026-10-24T00:00:00Z"))
                .unwrap()
        )
        .unwrap(),
        saved
    );
    lines(
        &claude_dir.join("a.jsonl"),
        &[claude("m1", "2026-10-23T01:00:00Z", 30, 0.7)],
    );
    assert_eq!(
        importer
            .scan(&sources)
            .overview(&selection, Utc::now())
            .unwrap()
            .summary
            .tokens
            .total(),
        430
    );
    fs::remove_file(codex_dir.join("rollout.jsonl")).unwrap();
    fs::remove_file(codex_dir.join("copy.jsonl")).unwrap();
    assert_eq!(
        importer
            .scan(&sources)
            .overview(&selection, Utc::now())
            .unwrap()
            .summary
            .tokens
            .total(),
        170
    );
    let mut invalid = selection;
    invalid.until = Some(time("2020-01-01T00:00:00Z"));
    assert!(data.overview(&invalid, Utc::now()).is_err());
    invalid.until = None;
    invalid.timezone = Some("unknown-zone".into());
    assert!(data.overview(&invalid, Utc::now()).is_err());
}
#[test]
fn reads_legacy_and_wal_database_without_writing_and_keeps_unknown_prices_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("zuse.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE projects(id TEXT,path TEXT); CREATE TABLE sessions(id TEXT,provider_id TEXT,model TEXT,project_id TEXT); CREATE TABLE messages(id TEXT,session_id TEXT,content_json TEXT,created_at TEXT,kind TEXT); INSERT INTO projects VALUES('p','/project'); INSERT INTO sessions VALUES('s','zuse','native-model','p');").unwrap();
    db.execute(
        "INSERT INTO messages VALUES('m1','s',?1,'2026-10-23T12:00:00Z','usage')",
        [json!({"inputTokens":"20","outputTokens":10}).to_string()],
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let sources = [Source::new(
        "zuse",
        "Zuse (Beta)",
        Format::Legacy,
        vec![path.clone()],
    )];
    let mut importer = Importer::default();
    let data = importer.scan(&sources);
    assert_eq!(data.records.len(), 1);
    assert_eq!(data.records[0].tokens.total(), 30);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        data.overview(&Selection::default(), Utc::now())
            .unwrap()
            .summary
            .cost_status,
        CostStatus::Unknown
    );
    db.execute(
        "INSERT INTO messages VALUES('m2','s',?1,'2026-10-23T13:00:00Z','usage')",
        [json!({"inputTokens":40,"outputTokens":5}).to_string()],
    )
    .unwrap();
    assert_eq!(
        importer.scan(&sources).records.len(),
        2,
        "A WAL update must invalidate the read without a main-file timestamp change"
    );
}
#[test]
fn reads_native_counts_only_with_recorded_timestamps_and_avoids_external_double_count() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.jsonl");
    lines(
        &path,
        &[
            json!({"seq":1,"event":{"type":"sessionStarted","harness":"zuse","model":"native-model","cwd":"/project"}}),
            json!({"seq":2,"at":"2026-10-23T12:00:00Z","event":{"type":"usage","inputTokens":10,"outputTokens":5}}),
            json!({"seq":3,"event":{"type":"usage","inputTokens":99,"outputTokens":99}}),
            json!({"seq":4,"event":{"type":"sessionStarted","harness":"claude","model":"claude-model","cwd":"/project"}}),
            json!({"seq":5,"at":"2026-10-23T12:00:00Z","event":{"type":"usage","inputTokens":55,"outputTokens":55}}),
        ],
    );
    let data = Importer::default().scan(&[Source::new(
        "zuse",
        "Zuse (Beta)",
        Format::Native,
        vec![temp.path().into()],
    )]);
    assert_eq!(data.records.len(), 1);
    assert_eq!(data.records[0].tokens.total(), 15);
    assert!(
        data.sources[0]
            .warning
            .as_deref()
            .unwrap()
            .contains("1 usage rows")
    );
}

#[test]
fn imports_other_supported_disk_formats_and_missing_sources_without_provider_calls() {
    let temp = tempfile::tempdir().unwrap();
    let opencode = temp.path().join("opencode");
    fs::create_dir_all(opencode.join("storage/message")).unwrap();
    let message = json!({"id":"oc-message","sessionID":"oc-session","providerID":"anthropic","modelID":"oc-model","time":{"created":1792756800000i64},"tokens":{"input":11,"output":12,"cache":{"read":13,"write":14},"reasoning":15},"cost":0.25});
    let mut older_export = message.clone();
    older_export["tokens"]["input"] = json!(999);
    fs::write(
        opencode.join("storage/message/message.json"),
        older_export.to_string(),
    )
    .unwrap();
    let db = rusqlite::Connection::open(opencode.join("opencode.db")).unwrap();
    db.execute_batch("CREATE TABLE message(id TEXT,session_id TEXT,data TEXT)")
        .unwrap();
    db.execute(
        "INSERT INTO message VALUES('oc-message','oc-session',?1)",
        [message.to_string()],
    )
    .unwrap();
    db.execute("INSERT INTO message VALUES('broken','broken','[]')", [])
        .unwrap();
    let amp = temp.path().join("amp");
    fs::create_dir(&amp).unwrap();
    fs::write(amp.join("thread.json"),json!({"id":"amp-session","usageLedger":{"events":[{"timestamp":"2026-10-23T12:00:00Z","model":"amp-model","toMessageId":1,"tokens":{"input":20,"output":10}}]},"messages":[{"role":"assistant","messageId":1,"usage":{"cacheReadInputTokens":4,"cacheCreationInputTokens":3}}]}).to_string()).unwrap();
    let pi = temp.path().join("pi");
    lines(
        &pi.join("session.jsonl"),
        &[
            json!({"type":"session","cwd":"/project/pi"}),
            json!({"timestamp":"2026-10-23T12:00:00Z","message":{"role":"assistant","model":"pi-model","usage":{"input":10,"output":5,"cacheRead":4,"cacheWrite":3,"cost":{"total":0.12}}}}),
        ],
    );
    let grok = temp.path().join("grok");
    lines(
        &grok.join("session/updates.jsonl"),
        &[
            json!({"timestamp":"2026-10-23T12:00:00Z","params":{"sessionId":"grok-session","update":{"sessionUpdate":"subagent_finished","tokens_used":30,"child_session_id":"child"}}}),
        ],
    );
    fs::write(
        grok.join("session/summary.json"),
        json!({"current_model_id":"grok-model","info":{"cwd":"/project/grok"}}).to_string(),
    )
    .unwrap();
    let sources = vec![
        Source::new("opencode", "OpenCode", Format::OpenCode, vec![opencode]),
        Source::new("amp", "Amp", Format::Amp, vec![amp]),
        Source::new("pi", "pi-agent", Format::Pi, vec![pi]),
        Source::new("grok", "Grok", Format::Grok, vec![grok]),
        Source::new(
            "claude",
            "Claude Code",
            Format::Claude,
            vec![temp.path().join("absent")],
        ),
    ];
    let data = Importer::default().scan(&sources);
    assert_eq!(
        data.records.len(),
        4,
        "Database and JSON copies of the same message must only count once"
    );
    let report = data.overview(&Selection::default(), Utc::now()).unwrap();
    assert_eq!(report.summary.tokens.total(), 65 + 37 + 22 + 30);
    assert_eq!(report.summary.cost_status, CostStatus::Partial);
    assert_eq!(report.summary.cost_usd, Some(0.37));
    assert_eq!(report.session_count, 4);
    assert!(
        !report
            .sources
            .iter()
            .find(|s| s.id == "claude")
            .unwrap()
            .detected
    );
    assert!(
        report
            .sources
            .iter()
            .find(|s| s.id == "grok")
            .unwrap()
            .warning
            .as_deref()
            .unwrap()
            .contains("subagent")
    );
}

#[test]
fn persisted_history_survives_export_removal_and_profile_changes_do_not_share_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("exports");
    let file = root.join("one.jsonl");
    lines(&file, &[claude("message", "2026-10-23T01:00:00Z", 20, 0.6)]);
    let sources = [Source::new(
        "claude",
        "Claude Code",
        Format::Claude,
        vec![root],
    )];
    let mut importer = Importer::default();
    let path = temp.path().join("usage.sqlite");
    {
        let mut ledger = zuse_usage::ledger::Ledger::open(&path).unwrap();
        let data = ledger.merge(importer.scan(&sources)).unwrap();
        assert_eq!(data.records.len(), 1);
        assert_eq!(data.records[0].cost_usd, Some(0.6));
    }
    fs::remove_file(&file).unwrap();
    let mut reopened = zuse_usage::ledger::Ledger::open(&path).unwrap();
    let retained = reopened.merge(importer.scan(&sources)).unwrap();
    assert_eq!(retained.records.len(), 1);
    assert_eq!(retained.records[0].tokens.total(), 160);
    assert_eq!(retained.sources[0].record_count, 1);
    let mut unknown = retained.clone();
    unknown.records[0].cost_usd = None;
    assert_eq!(
        reopened.merge(unknown).unwrap().records[0].cost_usd,
        Some(0.6),
        "An unpriced refresh must not erase a saved known price"
    );
    let separate = zuse_usage::ledger::Ledger::open(&temp.path().join("another-profile.sqlite"))
        .unwrap()
        .merge(importer.scan(&sources))
        .unwrap();
    assert!(separate.records.is_empty());
}
