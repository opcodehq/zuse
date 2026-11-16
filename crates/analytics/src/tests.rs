use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn expanded_stats_keep_variants_and_measurements_inside_the_closed_boundary() {
    assert_eq!(
        sanitize(
            "feature used",
            json!({
                "feature":"git action", "variant":"commit", "failure_kind":"operation",
                "outcome":"failure", "duration_ms":42, "message":"private", "branch":"private",
            })
        ),
        Some(
            json!({"feature":"git action","variant":"commit","failure_kind":"operation","outcome":"failure","duration_ms":42})
        )
    );
    let invalid = sanitize(
        "feature used",
        json!({
            "feature":"git action", "variant":"private command", "failure_kind":"private failure",
        }),
    )
    .unwrap();
    assert_eq!(invalid, json!({"feature":"git action"}));
    let report = sanitize(
        "turn interrupted",
        json!({
            "output_character_count":0, "reasoning_character_count":7,
            "context_tokens":0, "context_window_tokens":200000,
            "usage_report_version":1, "usage_status":"partial", "terminal_status":"interrupted",
            "service_tier":"fast", "input_tokens":5, "text":"private",
        }),
    )
    .unwrap();
    assert_eq!(report["context_tokens"], 0);
    assert_eq!(report["output_character_count"], 0);
    assert_eq!(report["usage_report_version"], 1);
    assert_eq!(report["usage_status"], "partial");
    assert_eq!(report["terminal_status"], "interrupted");
    assert_eq!(report["service_tier"], "fast");
    assert!(!report.to_string().contains("private"));
    assert_eq!(
        sanitize(
            "turn failed",
            json!({"usage_report_version":2,"terminal_status":"private","service_tier":"private"})
        ),
        Some(json!({}))
    );
    assert_eq!(
        sanitize(
            "onboarding step viewed",
            json!({"step":"agents","name":"private"})
        ),
        Some(json!({"step":"agents"}))
    );
    assert!(sanitize("onboarding step viewed", json!({"step":"private"})).is_none());
}

#[tokio::test]
async fn client_metadata_is_explicit_and_does_not_replace_engine_environment() {
    use desktop::{Client, ClientArchitecture, ClientOs, Event, Observation};
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "test".into(), url, true);
    let epoch = analytics.status().collection_epoch;
    analytics.capture_observation(Observation {
        collection_epoch: epoch.clone(),
        event: Event::Foregrounded,
        client: Some(Client {
            os: ClientOs::Windows,
            architecture: ClientArchitecture::X86_64,
            version: "1.2.3".into(),
        }),
    });
    analytics.capture_observation(Observation {
        collection_epoch: epoch.clone(),
        event: Event::PaletteOpened,
        client: Some(Client {
            os: ClientOs::Linux,
            architecture: ClientArchitecture::Aarch64,
            version: "private-build".into(),
        }),
    });
    analytics.capture_observation(Observation {
        collection_epoch: epoch,
        event: Event::Backgrounded,
        client: None,
    });
    analytics.shutdown().await;
    let payload = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap();
    let batch = payload["batch"].as_array().unwrap();
    assert_eq!(batch.len(), 3);
    assert_eq!(batch[0]["properties"]["client_os"], "windows");
    assert_eq!(batch[0]["properties"]["client_version"], "1.2.3");
    assert_eq!(batch[0]["properties"]["os"], std::env::consts::OS);
    assert!(batch[1]["properties"].get("client_version").is_none());
    assert!(batch[2]["properties"].get("client_os").is_none());
    assert!(!payload.to_string().contains("private"));
    let invalid = json!({"collectionEpoch":"x","event":{"event":"palette opened"},"client":{"os":"private","architecture":"x86_64","version":"1.2.3"}});
    assert!(serde_json::from_value::<Observation>(invalid).is_err());
}

#[test]
fn payload_boundary_rejects_content_and_unknown_events() {
    assert!(sanitize("untrusted title", json!({})).is_none());
    assert_eq!(
        sanitize(
            "turn completed",
            json!({
                "provider": "my private integration", "tool_count": 4,
                "tool_failure_count": -1, "duration_ms": 12,
                "prompt": "private", "path": "/home/private", "error": "secret",
                "input_tokens": "credential", "model": "private-model"
            })
        ),
        Some(json!({"tool_count": 4, "duration_ms": 12, "model": "custom"}))
    );
    assert!(feature("ReadWorkspaceFile", &json!({})).is_none());
    assert_eq!(
        feature("Mutate", &json!({"op": "createChat", "title": "secret"})),
        Some("chat created")
    );
    assert!(feature("Mutate", &json!({"op": "private"})).is_none());
}

fn request() -> zuse_protocol::RunRequest {
    serde_json::from_value(json!({"prompt": "private prompt", "model": "gpt-6-astra",
        "reasoning": "high", "cwd": "/private/repository", "sandbox": "workspace-write", "resume": null})).unwrap()
}

#[test]
fn vendor_models_are_scoped_and_custom_identifiers_are_redacted() {
    let models = HashMap::from([("codex".into(), HashSet::from(["gpt-6-astra".into()]))]);
    let valid = sanitize_with_models(
        "message submitted",
        json!({"provider": "codex", "model": "gpt-6-astra",
        "reasoning_effort": "high", "permission_mode": "workspace-write", "plan_mode": true}),
        &models,
    )
    .unwrap();
    assert_eq!(valid["model"], "gpt-6-astra");
    assert_eq!(valid["reasoning_effort"], "high");
    assert_eq!(valid["plan_mode"], true);
    for provider in ["acp", "opencode", "claude"] {
        let redacted = sanitize_with_models(
            "turn started",
            json!({"provider": provider, "model": "gpt-6-astra"}),
            &models,
        )
        .unwrap();
        assert_eq!(redacted["model"], "custom");
    }
    let omp = sanitize_with_models(
        "turn started",
        json!({"provider": "omp", "model": "default"}),
        &models,
    )
    .unwrap();
    assert_eq!(omp["provider"], "omp");
    let invalid = sanitize_with_models("turn completed", json!({"provider": "codex", "model": "company-private-model",
        "actual_model": "https://private/model", "reasoning_effort": "secret setting", "prompt": "private prompt"}), &models).unwrap();
    assert_eq!(invalid["model"], "custom");
    assert_eq!(invalid["actual_model"], "custom");
    assert!(!invalid.to_string().contains("private"));
}

#[test]
fn turn_counts_deduplicate_tools_and_separate_nested_usage() {
    use zuse_protocol::{AgentEvent, DoneStatus, HarnessId, ToolCall};
    let mut turn = turn::Turn::new(HarnessId::Codex, &request(), false, false);
    assert_eq!(turn.finish()["usage_status"], "unknown");
    assert!(turn.finish().get("input_tokens").is_none());
    let call = AgentEvent::ToolCall {
        id: "private-call-id".into(),
        call: ToolCall::Exec {
            command: "private command".into(),
        },
    };
    turn.observe(&call);
    turn.observe(&call);
    let result = AgentEvent::ToolResult {
        id: "private-call-id".into(),
        is_error: true,
        output: Some("private output".into()),
        diff: None,
    };
    turn.observe(&result);
    turn.observe(&result);
    turn.observe(&AgentEvent::Usage {
        input_tokens: 100,
        output_tokens: 20,
    });
    turn.observe(&AgentEvent::Usage {
        input_tokens: 120,
        output_tokens: 30,
    });
    turn.observe(&AgentEvent::Subagent {
        parent_tool_use_id: "private-subagent".into(),
        event: Box::new(AgentEvent::Usage {
            input_tokens: 900,
            output_tokens: 900,
        }),
    });
    turn.observe(&AgentEvent::Subagent {
        parent_tool_use_id: "private-subagent".into(),
        event: Box::new(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        }),
    });
    let result = turn.finish();
    assert_eq!(result["tool_count"], 1);
    assert_eq!(result["shell_tool_count"], 1);
    assert_eq!(result["tool_failure_count"], 1);
    assert_eq!(result["subagent_count"], 1);
    assert_eq!(result["subagent_completed_count"], 1);
    assert_eq!(result["input_tokens"], 120);
    assert_eq!(result["output_tokens"], 30);
    assert!(!result.to_string().contains("private"));
    let next = turn.restart(true).finish();
    assert_eq!(next["tool_count"], 0);
    assert_eq!(next["usage_status"], "unknown");
    assert_eq!(next["continuation"], true);
}

#[test]
fn native_usage_sums_requests_and_never_emits_accounting_identifiers() {
    use zuse_protocol::{AgentEvent, HarnessId};
    let mut turn = turn::Turn::new(HarnessId::Zuse, &request(), false, false);
    turn.observe(&AgentEvent::Usage {
        input_tokens: 100,
        output_tokens: 20,
    });
    turn.observe(&AgentEvent::Usage {
        input_tokens: 40,
        output_tokens: 10,
    });
    let accounting = AgentEvent::RequestUsage {
        details: json!({"requestId": "private-request-id", "retryIndex": 0,
        "connectionId": "private-account-id", "model": "private-model", "errorCode": "private-error-message",
        "usage": {"cachedTokens": 8, "cacheWriteTokens": 4, "reasoningTokens": 6} }),
    };
    turn.observe(&accounting);
    turn.observe(&accounting);
    let result = turn.finish();
    assert_eq!(result["input_tokens"], 140);
    assert_eq!(result["output_tokens"], 30);
    assert_eq!(result["request_attempt_count"], 1);
    assert_eq!(result["request_failure_count"], 1);
    assert_eq!(result["cache_read_tokens"], 8);
    assert_eq!(result["cache_creation_tokens"], 4);
    assert_eq!(result["reasoning_tokens"], 6);
    assert!(!result.to_string().contains("private"));
}

#[tokio::test]
async fn persisted_opt_out_and_account_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "test".into(), "http://127.0.0.1:1".into(), true);
    let original = analytics
        .0
        .state
        .lock()
        .unwrap()
        .preference
        .anonymous_id
        .clone();
    let install = analytics
        .0
        .state
        .lock()
        .unwrap()
        .preference
        .install_id
        .clone();
    analytics.set_account(Some("private-account"));
    let id = analytics.0.state.lock().unwrap().account.clone().unwrap();
    assert_eq!(id, account_id("private-account"));
    assert!(!id.contains("private-account"));
    analytics.set_account(None);
    assert_eq!(
        analytics.0.state.lock().unwrap().preference.install_id,
        install
    );
    assert_ne!(
        original,
        analytics.0.state.lock().unwrap().preference.anonymous_id
    );
    analytics.set_enabled(false).unwrap();
    analytics.shutdown().await;
    let reopened = Analytics::start(dir.path(), "test".into(), "http://127.0.0.1:1".into(), true);
    assert!(!reopened.status().enabled);
    reopened.shutdown().await;
}

#[tokio::test]
async fn malformed_consent_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("analytics.json"), b"broken").unwrap();
    let analytics = Analytics::start(dir.path(), "test".into(), "http://127.0.0.1:1".into(), true);
    assert!(!analytics.status().enabled);
    analytics.shutdown().await;
}

async fn server(status: &'static str) -> (String, tokio::task::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let body = loop {
            let mut buffer = [0; 8192];
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                }
            }
        };
        stream
            .write_all(
                format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}")
                    .as_bytes(),
            )
            .await
            .unwrap();
        body
    });
    (url, handle)
}

#[tokio::test]
async fn batch_delivery_preserves_identity_and_scrubs_payload() {
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "public-test-key".into(), url, true);
    analytics.register_vendor_models("codex", ["gpt-6-astra".to_owned()]);
    analytics.capture(
        "turn completed",
        json!({"provider": "codex", "model": "gpt-6-astra", "reasoning_effort": "high", "tool_count": 2, "prompt": "private prompt"}),
    );
    analytics.shutdown().await;
    let payload = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["api_key"], "public-test-key");
    let event = &payload["batch"][0];
    assert_eq!(event["event"], "turn completed");
    assert_eq!(event["properties"]["model"], "gpt-6-astra");
    assert_eq!(event["properties"]["reasoning_effort"], "high");
    assert!(
        event["properties"]["install_id"]
            .as_str()
            .unwrap()
            .starts_with("install_")
    );
    assert!(uuid::Uuid::parse_str(event["uuid"].as_str().unwrap()).is_ok());
    assert_eq!(event["properties"]["$process_person_profile"], false);
    assert_eq!(event["properties"]["analytics_schema_version"], 2);
    assert!(!payload.to_string().contains("private prompt"));
}

#[tokio::test]
async fn revocation_discards_queued_events_before_delivery() {
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "test".into(), url, true);
    analytics.capture("app opened", json!({}));
    analytics.capture_from("desktop", "palette opened", json!({}));
    analytics.set_enabled(false).unwrap();
    analytics.capture("app opened", json!({}));
    analytics.shutdown().await;
    assert!(!request.is_finished());
    request.abort();
}

#[tokio::test]
async fn missing_configuration_never_delivers() {
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "".into(), url, false);
    analytics.capture("app opened", json!({}));
    analytics.shutdown().await;
    assert!(!request.is_finished());
    request.abort();
}

#[test]
fn native_catalog_rejects_arbitrary_dimensions_and_bounds_time() {
    for (event, properties, expected) in [
        (
            "screen viewed",
            json!({"screen": "chat", "path": "/private", "provider": "codex", "chat_id": "secret"}),
            json!({"screen": "chat"}),
        ),
        (
            "settings viewed",
            json!({"section": "general", "label": "private"}),
            json!({"section": "general"}),
        ),
        (
            "palette action invoked",
            json!({"action": "content", "query": "private query"}),
            json!({"action": "content"}),
        ),
        (
            "surface selected",
            json!({"surface_kind": "terminal", "title": "private"}),
            json!({"surface_kind": "terminal"}),
        ),
        (
            "preference changed",
            json!({"preference": "default_model", "value": "private model"}),
            json!({"preference": "default_model"}),
        ),
        (
            "app active interval",
            json!({"active_seconds": 60, "idle": "private"}),
            json!({"active_seconds": 60}),
        ),
        ("app foregrounded", json!({"title": "private"}), json!({})),
        ("palette opened", json!({"query": "private"}), json!({})),
    ] {
        assert_eq!(sanitize(event, properties), Some(expected), "{event}");
    }
    for (event, properties) in [
        ("screen viewed", json!({"screen": "/private/project"})),
        (
            "palette action invoked",
            json!({"action": "private command"}),
        ),
        ("settings viewed", json!({"section": "private"})),
        ("surface selected", json!({"surface_kind": "private"})),
        ("app active interval", json!({"active_seconds": 61})),
        ("app active interval", json!({"active_seconds": 0})),
        ("app active interval", json!({"active_seconds": -1})),
    ] {
        assert!(sanitize(event, properties).is_none(), "{event}");
    }
}

#[tokio::test]
async fn native_and_engine_share_consent_identity_and_epoch_boundaries() {
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "test".into(), url, true);
    analytics.set_enabled(false).unwrap();
    analytics.capture_from("desktop", "palette opened", json!({}));
    analytics.set_enabled(true).unwrap();
    analytics.set_account(Some("private-account"));
    // A freshly enabled/account-switched collector must not accept old dwell.
    analytics.capture_from(
        "desktop",
        "app active interval",
        json!({"active_seconds": 60}),
    );
    analytics.capture_from(
        "desktop",
        "screen viewed",
        json!({"screen": "chat", "query": "private"}),
    );
    analytics.capture("app opened", json!({}));
    analytics.shutdown().await;
    let payload = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap();
    let batch = payload["batch"].as_array().unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0]["properties"]["surface"], "desktop");
    assert_eq!(batch[1]["properties"]["surface"], "engine");
    assert_eq!(batch[0]["distinct_id"], batch[1]["distinct_id"]);
    assert_eq!(
        batch[0]["properties"]["install_id"],
        batch[1]["properties"]["install_id"]
    );
    assert_eq!(batch[0]["distinct_id"], account_id("private-account"));
    assert!(!payload.to_string().contains("private"));
}

#[tokio::test]
async fn native_intervals_are_trimmed_to_current_identity_epoch() {
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "test".into(), url, true);
    analytics.0.state.lock().unwrap().identity_since =
        std::time::Instant::now() - Duration::from_secs(5);
    analytics.capture_from(
        "desktop",
        "app active interval",
        json!({"active_seconds": 60}),
    );
    analytics.shutdown().await;
    let payload = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["batch"][0]["properties"]["active_seconds"], 5);
}

#[tokio::test]
async fn account_switch_invalidates_native_events_alongside_engine_events() {
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let analytics = Analytics::start(dir.path(), "test".into(), url, true);
    analytics.set_account(Some("first-private-account"));
    analytics.capture_from("desktop", "palette opened", json!({}));
    analytics.capture("app opened", json!({}));
    analytics.set_account(Some("second-private-account"));
    analytics.capture_from("desktop", "settings viewed", json!({"section": "general"}));
    analytics.shutdown().await;
    let payload = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["batch"].as_array().unwrap().len(), 1);
    assert_eq!(payload["batch"][0]["event"], "settings viewed");
    assert_eq!(
        payload["batch"][0]["distinct_id"],
        account_id("second-private-account")
    );
}

#[tokio::test]
async fn delayed_native_observations_cannot_cross_collection_epochs() {
    use desktop::Event;
    let (url, request) = server("200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let previous = Analytics::start(dir.path(), "test".into(), url.clone(), true);
    let restarted_epoch = previous.status().collection_epoch;
    previous.shutdown().await;
    let analytics = Analytics::start(dir.path(), "test".into(), url, true);
    let before_disable = analytics.status().collection_epoch;
    analytics.set_enabled(false).unwrap();
    let disabled = analytics.status().collection_epoch;
    analytics.set_enabled(true).unwrap();
    let before_account = analytics.status().collection_epoch;
    analytics.set_account(Some("first-account"));
    let first_account = analytics.status().collection_epoch;
    analytics.set_account(Some("second-account"));
    analytics.0.state.lock().unwrap().identity_since =
        std::time::Instant::now() - Duration::from_secs(5);
    for stale in [
        "".to_owned(),
        restarted_epoch,
        before_disable,
        disabled,
        before_account,
        first_account,
    ] {
        analytics.capture_desktop(&stale, Event::PaletteOpened);
        analytics.capture_desktop(&stale, Event::ActiveInterval { active_seconds: 60 });
    }
    let current = analytics.status().collection_epoch;
    analytics.capture_desktop(&current, Event::PaletteOpened);
    analytics.capture_desktop(&current, Event::ActiveInterval { active_seconds: 3 });
    analytics.shutdown().await;
    let payload = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["batch"].as_array().unwrap().len(), 2);
    assert_eq!(
        payload["batch"][0]["distinct_id"],
        account_id("second-account")
    );
    assert!(!payload.to_string().contains(&current));
    assert_eq!(payload["batch"][1]["properties"]["active_seconds"], 3);
}

#[test]
fn older_status_does_not_authorize_native_observations() {
    let status: Status =
        serde_json::from_value(json!({"enabled": true, "configured": true})).unwrap();
    assert!(status.collection_epoch.is_empty());
}
