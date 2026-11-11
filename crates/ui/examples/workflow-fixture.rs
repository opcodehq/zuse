//! A running workflow board in the production shell, then one of its agents
//! opened in the right pane. Isolated data, no agent runs. Prints `board` and
//! `pane` to stdout as each state settles so an outside capture can time its
//! screenshots; ZUSE_FIXTURE_HOLD_MS holds each state (default 2500). With
//! ZUSE_FIXTURE_EXIT the fixture quits after the pane state; otherwise close
//! the window to exit. Under Xvfb, point VK_ICD_FILENAMES at lavapipe.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::sync::Arc;
use zuse_protocol::{
    ToolCall, WorkflowAgentProgress, WorkflowAgentState, WorkflowPhase, WorkflowProgress,
};
use zuse_ui::*;

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[allow(clippy::too_many_arguments)]
fn agent(
    index: u32,
    label: &str,
    phase: u32,
    state: WorkflowAgentState,
    started: Option<i64>,
    duration: Option<u64>,
    prompt: &str,
    result: Option<&str>,
) -> WorkflowAgentProgress {
    WorkflowAgentProgress {
        index,
        label: label.into(),
        state,
        phase_index: Some(phase),
        phase_title: None,
        model: Some("claude-sonnet-5-5".into()),
        started_at: started,
        duration_ms: duration,
        prompt_preview: Some(prompt.into()),
        result_preview: result.map(Into::into),
        error: None,
    }
}

fn progress(now: i64) -> WorkflowProgress {
    use WorkflowAgentState::*;
    let ago = |s: i64| Some(now - s * 1000);
    WorkflowProgress {
        phases: vec![
            WorkflowPhase {
                index: 0,
                title: "Review".into(),
            },
            WorkflowPhase {
                index: 1,
                title: "Verify".into(),
            },
            WorkflowPhase {
                index: 2,
                title: "Report".into(),
            },
        ],
        agents: vec![
            agent(
                0,
                "review:bugs",
                0,
                Done,
                ago(140),
                Some(61_000),
                "Review the changed files for **correctness bugs**. Focus on `crates/engine/src/sessions.rs` and the resume path.",
                Some(
                    "Found 2 issues:\n\n1. `resume()` drops the queued steer when the run is interrupted mid-turn.\n2. `SessionRow::touch` races with the liveness sweep.\n\nBoth reproduce with the `turn_quiesce` fixture.",
                ),
            ),
            agent(
                1,
                "review:perf",
                0,
                Done,
                ago(140),
                Some(48_000),
                "Review the changed files for performance regressions.",
                Some("No regressions. The new index lookup is O(1) per frame."),
            ),
            agent(
                2,
                "review:security",
                0,
                Done,
                ago(139),
                Some(73_000),
                "Review the changed files for security issues: input validation, path traversal, secrets.",
                Some(
                    "Clean: paths are canonicalized against the workspace root before every read.",
                ),
            ),
            agent(
                3,
                "verify:resume drops queued steer",
                1,
                Done,
                ago(64),
                Some(29_000),
                "Adversarially verify: resume() drops the queued steer when interrupted.",
                Some("Confirmed. `cancel_queued` is set on every interrupt, including resumes."),
            ),
            agent(
                4,
                "verify:touch races liveness sweep",
                1,
                Running,
                ago(35),
                None,
                "Adversarially verify: SessionRow::touch races with the liveness sweep under load.",
                None,
            ),
            agent(
                5,
                "verify:stale cache on rename",
                1,
                Queued,
                None,
                None,
                "Adversarially verify: the title cache keeps the old name after a rename.",
                None,
            ),
        ],
    }
}

/// Draw and present a frame now: virtual displays never ask for one, so an
/// outside capture would otherwise read an empty window.
fn present(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp) {
    window
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
            window.present_if_needed();
        })
        .ok();
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let hold = std::env::var("ZUSE_FIXTURE_HOLD_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(2500u64);
    let exit = std::env::var_os("ZUSE_FIXTURE_EXIT").is_some();
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zuse_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zuse_engine::default_registry()),
            zuse_protocol::HarnessId::ClaudeCode,
            None,
        )
    })?;
    core.workspace.create_chat(
        "workflow",
        None,
        Some(&core.device_id),
        None,
        Some(workspace.to_string_lossy().into_owned()),
    )?;
    core.workspace
        .rename_chat("workflow", "Review the session changes")?;
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
    let now = chrono::Utc::now().timestamp_millis();
    let call = ToolCall::workflow_run(
        Some(&json!({ "scriptPath": "/tmp/workflows/review-changes.js" })),
        Some("review-changes"),
        Some(&progress(now)),
    );
    let entries = json!([
        {"id": "u1", "role": "user", "createdAt": now - 150_000, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": "Review the session changes and verify every finding."}]},
        {"id": "a1", "role": "assistant", "createdAt": now - 149_000, "deviceId": device, "status": "complete",
         "parts": [
            {"id": "pa", "kind": "text", "text": "Two scouts map the change first."},
            {"id": "s1", "kind": "tool", "call": {"kind": "unknown", "name": "Agent", "input": {"description": "Map the session lifecycle"}},
             "isError": false, "resolved": true, "subagentRef": "fixture-sub-1", "subagentStatus": "done", "subagentTail": "Sessions park after each turn"},
            {"id": "s2", "kind": "tool", "call": {"kind": "unknown", "name": "Agent", "input": {"description": "List callers of resume()"}},
             "isError": false, "resolved": true, "subagentRef": "fixture-sub-2", "subagentStatus": "done", "subagentTail": "4 call sites"},
            {"id": "p0", "kind": "text", "text": "Starting a review workflow: three reviewers, then an adversarial check of each finding."},
            {"id": "wf", "kind": "tool", "call": call, "isError": false, "resolved": true, "subagentStatus": "running"},
         ]},
    ]);

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
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zuse_protocol::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Development);
                s.local_device_id = Some(device.clone());
                s.devices = vec![
                    serde_json::from_value(json!({"id": device, "name": "This device",
                    "platform": std::env::consts::OS, "lastSeenAt": null}))
                    .unwrap(),
                ];
                s.chats = chats;
                s.selected_chat = Some("workflow".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.no_project = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(0.), px(0.)),
                            size(px(1440.), px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            state.update(cx, |_, cx| cx.notify());
            cx.activate(true);
            cx.spawn(async move |cx| {
                let executor = cx.background_executor().clone();
                let pause = |ms| executor.timer(std::time::Duration::from_millis(ms));
                pause(800).await;
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zuse_doc::TranscriptFrame::Reset {
                            reset: serde_json::from_value(entries).unwrap(),
                        },
                        cx,
                    )
                    .unwrap();
                    cx.notify();
                });
                pause(1200).await;
                present(window.into(), cx);
                println!("board");
                for _ in 0..hold / 200 {
                    pause(200).await;
                    present(window.into(), cx);
                }
                let transcript = window
                    .update(cx, |shell, _, _| shell.fixture_transcript())
                    .unwrap();
                transcript.update(cx, |t, cx| t.fixture_open_workflow_agent(0, cx));
                pause(1200).await;
                present(window.into(), cx);
                println!("pane");
                for _ in 0..hold / 200 {
                    pause(200).await;
                    present(window.into(), cx);
                }
                if exit {
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
