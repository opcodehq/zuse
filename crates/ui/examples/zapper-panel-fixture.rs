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
        workers: vec![],
        decisions: vec![ZapperDecision {
            id: "decision-1".into(),
            worker_chat_id: None,
            question: "Rebuild the base image before the release?".into(),
            answer: None,
        }],
        issues: vec![
            {
                let mut asked = issue(6, "Improve Zappers", ZapperIssueStatus::NeedsYou);
                asked.activity.push(ZapperIssueActivity {
                    actor: Some("Release".into()),
                    id: "question-1".into(),
                    created_at: asked.created_at,
                    kind: "question".into(),
                    text: "Should the Overview list only what waits on you?".into(),
                    reply_to: None,
                });
                asked
            },
            issue(7, "Zapper panel: closable tabs", ZapperIssueStatus::Todo),
        ],
        folders: vec![
            "handoffs".into(),
            "handoffs/design-references".into(),
            "reports".into(),
        ],
    }
}

fn files() -> Vec<ZapperFile> {
    let file = |path: &str, version, content: &str| ZapperFile {
        path: path.into(),
        version,
        content: content.into(),
        deleted: false,
    };
    vec![
        file(
            "AGENTS.md",
            2,
            "# Rules\n\n- Ask before opening pull requests that touch billing.\n- Keep threads to one issue each.",
        ),
        file(
            "handoffs/conversational-pr-research.md",
            1,
            "# Research\n\nNotes from the conversational review study.",
        ),
        file("handoffs/design-references/composer.md", 2, "# Composer"),
        file("handoffs/design-references/sidebar.md", 1, "# Sidebar"),
        file("reports/2026-11-26.md", 1, "# Weekly report"),
        file("INSTRUCTIONS.md", 2, "I keep the release train moving."),
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
                // ZAPPER_FIXTURE_LIGHT captures the light theme.
                if std::env::var_os("ZAPPER_FIXTURE_LIGHT").is_some() {
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
                            // ZAPPER_FIXTURE_WIDTH captures a narrower window.
                            size(
                                px(std::env::var("ZAPPER_FIXTURE_WIDTH")
                                    .ok()
                                    .and_then(|w| w.parse().ok())
                                    .unwrap_or(1180.)),
                                px(820.),
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
