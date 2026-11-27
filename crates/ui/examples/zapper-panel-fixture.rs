//! Native visual evidence for the Zapper panel inside the production shell,
//! with an isolated engine and seeded offline data. No agent messages are
//! sent. Usage: `zapper-panel-fixture <capture directory>` (X11 display).
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zuse_protocol::zappers::*;
use zuse_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.refresh();
        w.draw(cx).clear();
    })?;
    pause(cx, 400).await;
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/private-secrets-fixture/capture.py"
        ))
        .arg(std::process::id().to_string())
        .arg(directory.join(format!("{name}.png")))
        .status()?;
    anyhow::ensure!(status.success(), "fixture window capture failed");
    Ok(())
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

const COORDINATOR: &str = "zapper-release-0";
const WORKER: &str = "zapper-worker-deploy";

/// `ZAPPER_FIXTURE_CONVERSATION=1`: an issue shared by the Zapper and one
/// thread, answered by the person, with replies, quiet events and delivery
/// states, shaped like what the engine stores. `=error` adds a held reply,
/// `=queued` one waiting for the thread's turn to end.
fn conversation(now: i64, mode: &str) -> ZapperIssue {
    let at = |minutes: i64| now - minutes * 60_000;
    let change = |field: &str, before: Option<&str>, after: Option<&str>| ZapperIssueChange {
        field: field.into(),
        before: before.map(Into::into),
        after: after.map(Into::into),
    };
    let entry =
        |id: &str, actor: Option<&str>, minutes: i64, kind: &str, text: &str| ZapperIssueActivity {
            actor: actor.map(Into::into),
            id: id.into(),
            created_at: at(minutes),
            kind: kind.into(),
            text: text.into(),
            ..Default::default()
        };
    let delivered = |reply: &str| ZapperIssueDelivery {
        chat_id: WORKER.into(),
        state: ZapperIssueDeliveryState::Answered,
        detail: None,
        reply: Some(reply.into()),
    };
    let mut activity = vec![
        entry(
            "created",
            Some(COORDINATOR),
            50,
            "created",
            "Created the issue",
        ),
        ZapperIssueActivity {
            changes: vec![
                change("thread", None, Some(WORKER)),
                change("owner", Some("zapper"), Some(WORKER)),
                change("status", Some("todo"), Some("inProgress")),
            ],
            ..entry(
                "delegate",
                Some(COORDINATOR),
                49,
                "thread",
                &format!("Started thread {WORKER} · In progress"),
            )
        },
        ZapperIssueActivity {
            changes: vec![change("priority", Some("normal"), Some("high"))],
            ..entry(
                "priority",
                Some(COORDINATOR),
                48,
                "updated",
                "Priority changed to High",
            )
        },
        ZapperIssueActivity {
            thread: Some(WORKER.into()),
            changes: vec![
                change("owner", Some(WORKER), Some("user")),
                change("status", Some("inProgress"), Some("needsYou")),
            ],
            ..entry(
                "ask",
                Some(COORDINATOR),
                30,
                "question",
                "Should code.zuse.sh connect to engines through the same-origin proxy (A) or with T3-style tokens (B)?\n\nRecommendation: B, so the app needs no proxy.",
            )
        },
        ZapperIssueActivity {
            delivery: Some(delivered("report-1")),
            ..entry(
                "comment-1",
                None,
                9,
                "comment",
                "btw both solid and main landing will be different dude, solid js will be code.zuse.sh and landing will be zuse.sh",
            )
        },
        ZapperIssueActivity {
            reply_to: Some("comment-1".into()),
            ..entry(
                "report-1",
                Some(WORKER),
                8,
                "workerReport",
                "Worker update\nGot it: code.zuse.sh for the Solid app, zuse.sh for the landing page. I'll build the Cloudflare deploy for code.zuse.sh now: a Worker with static assets, SPA fallback, caching, the current security headers, and a deploy workflow from main.",
            )
        },
        ZapperIssueActivity {
            delivery: Some(delivered("report-2")),
            ..entry("comment-2", None, 4, "comment", "yeah basically b")
        },
        ZapperIssueActivity {
            reply_to: Some("comment-2".into()),
            ..entry(
                "report-2",
                Some(WORKER),
                3,
                "workerReport",
                "Worker update\nB it is. I'll split it in two:\n1. The Cloudflare deploy for code.zuse.sh, one PR with no app changes.\n2. A short scope for the token-based connection before anything gets built.\nBoth will be posted here.",
            )
        },
        ZapperIssueActivity {
            changes: vec![change("status", Some("needsYou"), Some("inProgress"))],
            ..entry(
                "moved",
                Some(COORDINATOR),
                2,
                "updated",
                "Status changed to In progress",
            )
        },
        ZapperIssueActivity {
            changes: vec![change("owner", Some("user"), Some(WORKER))],
            ..entry(
                "assigned",
                Some(COORDINATOR),
                2,
                "updated",
                &format!("Owner: {WORKER}"),
            )
        },
        entry(
            "summary",
            Some(COORDINATOR),
            1,
            "comment",
            &format!(
                "Swaraj picked B: a standalone app on code.zuse.sh that connects to engines with tokens. {WORKER} is building the Cloudflare deploy PR first, and will write the scope for the cross-origin connection work before building any of it."
            ),
        ),
    ];
    if mode == "error" || mode == "queued" {
        let (state, detail) = if mode == "error" {
            (
                ZapperIssueDeliveryState::Held,
                Some("You stopped this thread. Continue it from its conversation first".into()),
            )
        } else {
            (ZapperIssueDeliveryState::Queued, None)
        };
        activity.push(ZapperIssueActivity {
            delivery: Some(ZapperIssueDelivery {
                chat_id: WORKER.into(),
                state,
                detail,
                reply: None,
            }),
            reply_to: Some("report-2".into()),
            ..entry(
                "comment-3",
                None,
                0,
                "comment",
                "Also add a preview deploy for pull requests",
            )
        });
    }
    ZapperIssue {
        id: "issue-21".into(),
        number: 21,
        details: ZapperIssueDetails {
            title: "Deploy the Solid web client on Cloudflare".into(),
            description: "Serve apps/web-solid at code.zuse.sh, separate from the zuse.sh landing page.".into(),
            status: ZapperIssueStatus::InProgress,
            priority: ZapperIssuePriority::High,
            assigned: true,
            owner: Some(ZapperIssueOwner::Thread { chat_id: WORKER.into() }),
            done_criteria: "code.zuse.sh serves the Solid app from main with SPA fallback and security headers.".into(),
            ..Default::default()
        },
        created_at: at(50),
        updated_at: at(1),
        threads: vec![WORKER.into()],
        pull_requests: vec![],
        activity,
    }
}

fn issue(number: u32, title: &str, status: ZapperIssueStatus) -> ZapperIssue {
    let now = chrono::Utc::now().timestamp_millis();
    ZapperIssue {
        id: format!("issue-{number}"),
        number,
        details: ZapperIssueDetails {
            title: title.into(),
            description: "Make the flow work end to end and verify it on staging.".into(),
            status,
            priority: ZapperIssuePriority::High,
            assigned: true,
            owner: Some(ZapperIssueOwner::User),
            done_criteria: "Works end to end on staging with a passing smoke test.".into(),
            ..Default::default()
        },
        created_at: now - 54_000_000,
        updated_at: now - i64::from(number) * 3_600_000,
        threads: vec![],
        pull_requests: vec![],
        activity: vec![],
    }
}

fn sample_zapper(device: &str) -> Zapper {
    Zapper {
        id: "release".into(),
        revision: 4,
        activity_revision: 0,
        config: ZapperConfig {
            name: "Release".into(),
            purpose: "Keep staging healthy and ship the cloud chat work.".into(),
            home_device_id: device.into(),
            repositories: vec![],
            coordinator_model: ZapperModel {
                harness: "claude-code".into(),
                model: None,
            },
            worker_device_id: device.into(),
            max_concurrent_workers: 3,
            max_followups: 3,
            avatar: Some(ZapperAvatar {
                mascot: "bolt".into(),
                color: "violet".into(),
            }),
            avatar_image: None,
        },
        status: ZapperStatus::Active,
        coordinator_chat_id: COORDINATOR.into(),
        previous_coordinator_chats: vec![],
        workers: match std::env::var("ZAPPER_FIXTURE_CONVERSATION") {
            Ok(_) => vec![ZapperWorker {
                chat_id: WORKER.into(),
                device_id: device.into(),
                space_id: "web".into(),
                objective: "Deploy the Solid web client on Cloudflare".into(),
                followups: 0,
                queue_position: None,
            }],
            Err(_) => vec![],
        },
        decisions: vec![ZapperDecision {
            id: "decision-1".into(),
            worker_chat_id: None,
            question: "Rebuild the base image before the release?".into(),
            answer: None,
        }],
        issues: if let Ok(mode) = std::env::var("ZAPPER_FIXTURE_CONVERSATION") {
            vec![conversation(chrono::Utc::now().timestamp_millis(), &mode)]
        } else {
            vec![
                {
                    let mut asked = issue(6, "Improve Zappers", ZapperIssueStatus::NeedsYou);
                    asked.activity.push(ZapperIssueActivity {
                        actor: Some("Release".into()),
                        id: "question-1".into(),
                        created_at: asked.created_at,
                        kind: "question".into(),
                        text: "Should the Overview list only what waits on you?".into(),
                        ..Default::default()
                    });
                    asked
                },
                issue(7, "Zapper panel: closable tabs", ZapperIssueStatus::Todo),
            ]
        },
    }
}

fn files() -> Vec<ZapperFile> {
    let file = |path: &str, version, content: &str| ZapperFile {
        path: path.into(),
        version,
        content: content.into(),
    };
    vec![
        file(
            "DIGEST.md",
            2,
            "# Digest: Release (2026-11-20)\n\n- Staging is healthy.\n- **Two** issues are open; one needs you.\n\n## Next\n\n1. Ship the panel tabs.\n2. Rebuild the base image.",
        ),
        file("NOTES.md", 1, "# Notes\n\nKeep threads small and verified."),
        file(
            "WORKERS.md",
            3,
            "# Workers / decisions log\n\n- panel-tabs: running",
        ),
    ]
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    let steps: Vec<String> = std::env::args().skip(2).collect();
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zuse_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zuse_engine::default_registry()),
            zuse_protocol::HarnessId::ClaudeCode,
            None,
        )
    })?;
    core.workspace
        .create_chat(COORDINATOR, None, Some(&core.device_id), None, None)?;
    core.workspace.rename_chat(COORDINATOR, "Release")?;
    core.workspace
        .create_chat(WORKER, None, Some(&core.device_id), None, None)?;
    core.workspace.rename_chat(
        WORKER,
        "Deploy Solid web client (apps/web-solid) on Cloudflare",
    )?;
    let ipc_port = port();
    let _ipc = runtime.block_on(zuse_engine::serve_ipc(ipc_port, core.rpc_service()))?;
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let boot = EngineBootConfig {
        target: None,
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zuse_protocol::HarnessId::ClaudeCode,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let chats = core.workspace.read_chats()?;
    let device = core.device_id.clone();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let settings = settings::UiSettings::default();
            settings::init(settings.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                settings.ui_font_family.clone(),
                settings.ui_font_size,
                settings.terminal_font_family.clone(),
                settings.terminal_font_size,
                settings.code_font_family.clone(),
                settings.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data.clone(), cx);
            appearance::init(
                if std::env::var("ZAPPER_FIXTURE_LIGHT").is_ok() {
                    appearance::AppearanceMode::Light
                } else {
                    appearance::AppearanceMode::Dark
                },
                settings.theme_selection,
                settings.accent,
                settings.surface,
                cx,
            );
            history::init(
                settings.git_history_columns,
                settings.git_history_column_widths,
                settings.git_history_column_order,
                settings.git_history_author_display,
                cx,
            );
            composer::init(cx, settings.composer_send_behavior);
            terminal::panel::init(cx);
            app_menus::init(cx);
            let zapper = sample_zapper(&device);
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zuse_protocol::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Development);
                s.local_device_id = Some(device.clone());
                s.devices = vec![serde_json::from_value(serde_json::json!({"id":device,"name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];
                s.chats = chats;
                s.zappers = vec![zapper];
                // The worker reports a fresh running turn, as a live session would.
                if std::env::var("ZAPPER_FIXTURE_CONVERSATION").is_ok() {
                    s.sessions = vec![serde_json::from_value(serde_json::json!({
                        "chatId": WORKER, "deviceId": device, "status": "working",
                        "startedAt": chrono::Utc::now().to_rfc3339(), "updatedAt": chrono::Utc::now().to_rfc3339(),
                        "runningSubagents": 0, "backgroundTasks": []
                    })).unwrap()];
                }
                s.selected_chat = Some(COORDINATOR.into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(40.)),
                            size(
                                px(std::env::var("ZAPPER_FIXTURE_WIDTH")
                                    .ok()
                                    .and_then(|w| w.parse().ok())
                                    .unwrap_or(1180.)),
                                px(std::env::var("ZAPPER_FIXTURE_HEIGHT")
                                    .ok()
                                    .and_then(|h| h.parse().ok())
                                    .unwrap_or(820.)),
                            ),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            state.update(cx, |_, cx| cx.notify());
            // No activation: the capture reads this window only, and the
            // fixture must not take keyboard focus from the desktop.
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    pause(cx, 1500).await;
                    for step in &steps {
                        window.update(cx, |s, _, cx| s.fixture_zapper_panel(files(), step, cx))?;
                        pause(cx, 900).await;
                        window.update(cx, |s, _, cx| s.fixture_zapper_settle(step, cx))?;
                        pause(cx, 300).await;
                        capture(window.into(), cx, &output, step).await?;
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("Zapper panel fixture failed: {error:#}");
                    *result.lock().unwrap() = Some(format!("{error:#}"));
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
