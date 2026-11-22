//! Zapper threads beyond `maxConcurrentWorkers` in the production shell:
//! the sidebar with a hovered thread row (its archive button), then a queued
//! thread opened. Isolated engine, seeded offline data, no agent runs, and the
//! window never takes focus. Usage: `HOVER_X=.. HOVER_Y=.. zapper-queue-fixture
//! <capture directory>` (X11 display). Builds against older shells too, so the
//! same fixture captures before and after.
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zuse_protocol::zots::*;
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

const COORDINATOR: &str = "zot-release-0";

/// (chat, title, objective, queue position)
const THREADS: [(&str, &str, &str, Option<u32>); 4] = [
    (
        "zot-worker-tabs",
        "Zapper Panel Tab Chrome",
        "Panel tabs close and reorder.",
        None,
    ),
    (
        "zot-worker-avatars",
        "Zapper Avatar Redesign",
        "Redraw the Zapper avatars.",
        None,
    ),
    (
        "zot-worker-errors",
        "Fix ZAP-11 (zapper-command-errors)",
        "ZAP-11: make Zapper commands report what failed and how to fix it. Read the issue, reproduce the failing delegate and messageWorker calls, fix them, add tests, and open a draft PR.",
        Some(1),
    ),
    (
        "zot-worker-icons",
        "ZAP-12 (zapper-tool-icons)",
        "ZAP-12: give Zapper tool calls their own icon.",
        Some(2),
    ),
];

fn sample_zapper(device: &str) -> Zot {
    // JSON so older protocol versions (without queuePosition) build too.
    let workers: Vec<_> = THREADS
        .iter()
        .map(|(chat, _, objective, queued)| {
            serde_json::json!({
                "chatId": chat, "deviceId": device, "spaceId": "project",
                "objective": objective, "followups": 0, "queuePosition": queued,
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "id": "release", "revision": 4,
        "config": {
            "name": "zappers stuff", "purpose": "Ship the Zapper work.",
            "homeDeviceId": device, "repositories": [],
            "coordinatorModel": {"harness": "claude-code", "model": null},
            "workerDeviceId": device, "maxConcurrentWorkers": 2, "maxFollowups": 3,
            "avatar": {"mascot": "ace", "color": "violet"}
        },
        "status": "active", "coordinatorChatId": COORDINATOR,
        "previousCoordinatorChats": [], "decisions": [], "issues": [],
        "workers": workers,
    }))
    .unwrap()
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    let hover: (f32, f32) = (
        std::env::var("HOVER_X")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120.),
        std::env::var("HOVER_Y")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(269.),
    );
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
    // Threads that started have chats; queued ones don't exist yet.
    for (chat, title, _, queued) in THREADS {
        if queued.is_none() {
            core.workspace
                .create_chat(chat, None, Some(&core.device_id), None, None)?;
            core.workspace.rename_chat(chat, title)?;
        }
    }
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
            let zot = sample_zapper(&device);
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zuse_protocol::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Development);
                s.local_device_id = Some(device.clone());
                s.devices = vec![serde_json::from_value(serde_json::json!({"id":device,"name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];
                s.chats = chats;
                s.zots = vec![zot];
                s.selected_chat = Some(THREADS[0].0.into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            let fixture_state = state.clone();
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(40.)),
                            size(px(1180.), px(820.)),
                        ))),
                        // Never take keyboard focus from the desktop.
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
                    // Hover a thread row with a synthetic in-window pointer
                    // move (the real pointer is left alone).
                    window.update(cx, |_, w, cx| {
                        w.dispatch_event(
                            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                position: gpui::point(px(hover.0), px(hover.1)),
                                pressed_button: None,
                                modifiers: Default::default(),
                            }),
                            cx,
                        );
                    })?;
                    pause(cx, 600).await;
                    capture(window.into(), cx, &output, "sidebar-hover").await?;
                    // What opening the queued row selects.
                    fixture_state.update(cx, |s, cx| s.select_chat(Some(THREADS[2].0.into()), cx));
                    pause(cx, 900).await;
                    capture(window.into(), cx, &output, "queued-open").await?;
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("Zapper panel fixture failed: {error:#}");
                    *result.lock().unwrap() = Some(format!("{error:#}"));
                }
                drop(fixture_state);
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
