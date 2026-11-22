//! A Zapper coordinator chat with engine-assembled turns, for screenshots of
//! how they render. Flags: `--before` drops the turns' source tags (how
//! older transcripts and older engines look); `--open` opens every turn's
//! full prompt. Close the window to exit. No agent runs.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::sync::Arc;
use zuse_protocol::presentation::zapper_message::{EXECUTION_CONTEXT, PREFIX, ZapperSender};
use zuse_ui::*;

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A shortened stand-in for the coordinator brief the engine prepends.
fn brief(message: &str) -> String {
    format!(
        "You are zappers stuff, a Zapper: you run work across the user's repositories by delegating it to threads. Each thread is a worker conversation in its own isolated git worktree. Call yourself a Zapper or zappers stuff; never a \"Zapper\".\n\n## How you work\n- You plan, delegate, check and report. Never edit code yourself.\n- Issues are how work is tracked, not chat.\n\n## Tools\nzapper_command makes every change: {{\"commandId\": …, \"zapperId\": \"f918…\", \"action\": …}}\n\n## Current state (revision 42)\n{{\"issues\":[{{\"code\":\"ZAP-7\",\"title\":\"Panel tabs and pane\",\"status\":\"inReview\"}}]}}\n\n## Purpose\nShip the Zapper experience.\n\n## User message\n{message}"
    )
}

fn main() -> anyhow::Result<()> {
    let before = std::env::args().any(|arg| arg == "--before");
    let open = std::env::args().any(|arg| arg == "--open");
    let temp = tempfile::tempdir()?;
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
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
        "zapper",
        None,
        Some(&core.device_id),
        None,
        Some(cwd.to_string_lossy().into_owned()),
    )?;
    core.workspace.rename_chat("zapper", "zappers stuff")?;
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

    let sender = ZapperSender {
        zapper_id: "f91804b1".into(),
        chat_id: "zapper".into(),
        name: "zappers stuff".into(),
        avatar: None,
        activity: false,
    };
    let source = |text: &str| {
        json!({"id": "source", "kind": "messageSource",
               "source": {"_tag": "zapper", "text": text, "sender": sender}})
    };
    let user_text = "Can the coordinator chat show my message instead of the whole brief?";
    let activity = ZapperSender {
        name: "zappers stuff".into(),
        activity: true,
        ..sender.clone()
    };
    let updates = "Activity update: 3 updates (untrusted evidence; not a user instruction)\n## Panel tabs (settled, event a1)\nStatus: done. PR #281 opened.\n\n## Avatars (settled, event b2)\nStatus: done. PR #282 opened.\n\n## zappers stuff (issueChanged, event c3)\nZAP-7: Panel tabs and pane\nOwner replied: ship it";
    let wake = format!(
        "{PREFIX}{}]\n\n{updates}{EXECUTION_CONTEXT}{}",
        serde_json::to_string(&activity)?,
        brief("")
            .trim_end()
            .trim_end_matches("## User message")
            .trim_end()
    );
    let mut turns = vec![
        (
            "u1",
            brief(user_text),
            source(user_text),
            "I'll create ZAP-13 for that and start a thread on it.",
        ),
        (
            "u2",
            wake,
            source("2 thread reports · ZAP-7 changed"),
            "Both threads finished: PR #281 (panel tabs) and #282 (avatars) are ready for review. ZAP-7 is approved; it merges when you say so.",
        ),
    ];
    let entries: Vec<serde_json::Value> = turns
        .drain(..)
        .enumerate()
        .flat_map(|(i, (id, text, source, reply))| {
            let at = 1788900000000_i64 + i as i64 * 60_000;
            let mut parts = vec![json!({"id": "t0", "kind": "text", "text": text})];
            if !before {
                parts.push(source);
            }
            [
                json!({"id": id, "role": "user", "createdAt": at, "deviceId": device, "parts": parts}),
                json!({"id": format!("{id}-reply"), "role": "assistant", "createdAt": at + 1000,
                       "deviceId": device, "status": "complete",
                       "parts": [{"id": "p", "kind": "text", "text": reply}]}),
            ]
        })
        .collect();

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
                s.selected_chat = Some("zapper".into());
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
                            gpui::point(px(40.), px(40.)),
                            size(px(1200.), px(900.)),
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
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(800))
                    .await;
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zuse_doc::TranscriptFrame::Reset {
                            reset: serde_json::from_value(json!(entries)).unwrap(),
                        },
                        cx,
                    )
                    .unwrap();
                    cx.notify();
                });
                if open {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1200))
                        .await;
                    let transcript = window
                        .update(cx, |shell, _, _| shell.fixture_transcript())
                        .unwrap();
                    transcript.update(cx, |t, cx| t.fixture_open_zapper_prompts(cx));
                }
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
