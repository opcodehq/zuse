//! Native visual evidence that threads a Zapper started stay out of the
//! Projects sidebar, running or released, and stay reachable from the
//! Zapper's Threads tab. Offline: no engine, no agent messages. Never takes
//! keyboard focus. Usage: `zapper-threads-fixture <capture directory> [steps]`
//! with steps `sidebar` (an ordinary chat open) and `threads` (the Zapper's
//! Threads tab, Finished expanded). Needs an X11 display.
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zuse_protocol::zappers::*;
use zuse_ui::*;

const COORDINATOR: &str = "zapper-stuff-0";
const DEVICE: &str = "local";

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.refresh();
        w.draw(cx).clear();
        w.present_if_needed();
    })?;
    pause(cx, 400).await;
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/private-secrets-fixture/capture.py"
        ))
        .arg(std::process::id().to_string())
        .arg(path)
        .status()?;
    anyhow::ensure!(status.success(), "fixture window capture failed");
    Ok(())
}

fn chat(id: &str, title: &str, minutes_ago: i64) -> zuse_protocol::Chat {
    let created = chrono::Utc::now() - chrono::Duration::minutes(minutes_ago);
    serde_json::from_value(serde_json::json!({
        "id": id, "deviceId": DEVICE, "spaceId": "zuse", "title": title, "archived": false,
        "createdAt": created, "lastMessageAt": created,
        "config": {"harness": "claude-code", "model": "claude-opus-5-5", "reasoning": null,
            "sandbox": "workspace-write"}
    }))
    .unwrap()
}

/// Threads the owner's screenshot showed leaking, all released.
const RELEASED: [(&str, &str); 3] = [
    ("zapper-worker-delegate-zap11-ci-fix-1", "Get PR281 Green"),
    (
        "zapper-worker-delegate-command-errors-1",
        "Fix Zapper Commands",
    ),
    ("zapper-worker-delegate-panel-tabs-1", "Zapper Panel Tabs"),
];
const RUNNING: (&str, &str) = (
    "zapper-worker-delegate-threads-leak-1",
    "Keep threads in Zapper",
);

fn issue(number: u32, title: &str, status: ZapperIssueStatus, threads: &[&str]) -> ZapperIssue {
    let now = chrono::Utc::now().timestamp_millis();
    ZapperIssue {
        id: format!("issue-{number}"),
        number,
        details: ZapperIssueDetails {
            title: title.into(),
            status,
            assigned: true,
            owner: Some(ZapperIssueOwner::Zapper),
            ..Default::default()
        },
        created_at: now - 54_000_000,
        updated_at: now,
        threads: threads.iter().map(|t| (*t).to_owned()).collect(),
        pull_requests: vec![],
        activity: vec![],
    }
}

fn sample_zapper() -> Zapper {
    Zapper {
        id: "stuff".into(),
        revision: 9,
        activity_revision: 0,
        config: ZapperConfig {
            name: "zappers stuff".into(),
            purpose: "Make Zappers solid.".into(),
            home_device_id: DEVICE.into(),
            repositories: vec![ZapperRepository {
                device_id: DEVICE.into(),
                space_id: "zuse".into(),
            }],
            coordinator_model: ZapperModel {
                harness: "claude-code".into(),
                model: None,
            },
            worker_device_id: DEVICE.into(),
            max_concurrent_workers: 3,
            max_followups: 3,
            avatar: Some(ZapperAvatar {
                mascot: "ace".into(),
                color: "violet".into(),
            }),
        },
        status: ZapperStatus::Active,
        coordinator_chat_id: COORDINATOR.into(),
        previous_coordinator_chats: vec![],
        // Only the running thread is still a worker; release removed the rest.
        workers: vec![ZapperWorker {
            chat_id: RUNNING.0.into(),
            device_id: DEVICE.into(),
            space_id: "zuse".into(),
            objective: RUNNING.1.into(),
            followups: 0,
            queue_position: None,
        }],
        decisions: vec![],
        issues: vec![
            issue(
                11,
                "Zapper commands keep failing",
                ZapperIssueStatus::Done,
                &[RELEASED[0].0, RELEASED[1].0],
            ),
            issue(
                7,
                "Zapper panel tabs",
                ZapperIssueStatus::InReview,
                &[RELEASED[2].0],
            ),
            issue(
                19,
                "Finished Zapper threads leak",
                ZapperIssueStatus::InProgress,
                &[RUNNING.0],
            ),
        ],
    }
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    let mut steps: Vec<String> = std::env::args().skip(2).collect();
    if steps.is_empty() {
        steps = vec!["sidebar".into(), "threads".into()];
    }
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let project = data.join("zuse");
    std::fs::create_dir_all(&project)?;
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.sidebar_organization = settings::SidebarOrganization::ByProject;
            settings.sidebar_width = 300.0;
            settings.save(&data).unwrap();
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
                appearance::AppearanceMode::Dark,
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
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.connection = zuse_protocol::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
                s.local_device_id = Some(DEVICE.into());
                s.devices = vec![serde_json::from_value(serde_json::json!({"id":DEVICE,"name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];
                s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"zuse","deviceId":DEVICE,"path":project,"createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
                let mut chats = vec![
                    chat("plain-1", "Polish the command palette", 20),
                    chat("plain-2", "Fix authentication redirects", 95),
                    chat(RUNNING.0, RUNNING.1, 30),
                    chat(COORDINATOR, "zappers stuff", 600),
                ];
                for (n, (id, title)) in RELEASED.iter().enumerate() {
                    chats.push(chat(id, title, 120 + 60 * n as i64));
                }
                s.chats = chats;
                s.zappers = vec![sample_zapper()];
                s.selected_chat = Some("plain-1".into());
                s.selected_space = Some("zuse".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            let boot = EngineBootConfig {
                target: None,
                data_dir: data.clone(),
                ipc_port: 0,
                edge_url: String::new(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: HarnessId::ClaudeCode,
            };
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(40.)),
                            size(px(1180.), px(760.)),
                        ))),
                        // Evidence only: never take keyboard focus from the desktop.
                        focus: false,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            state.update(cx, |_, cx| cx.notify());
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    pause(cx, 1500).await;
                    for step in &steps {
                        if step == "threads" {
                            state.update(cx, |s, cx| {
                                s.selected_chat = Some(COORDINATOR.into());
                                cx.notify();
                            });
                            pause(cx, 300).await;
                            window.update(cx, |s, _, cx| s.fixture_zapper_panel(vec![], step, cx))?;
                        }
                        pause(cx, 900).await;
                        capture(window.into(), cx, &output.join(format!("{step}.png"))).await?;
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("Zapper threads fixture failed: {error:#}");
                    *result.lock().unwrap() = Some(format!("{error:#}"));
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
