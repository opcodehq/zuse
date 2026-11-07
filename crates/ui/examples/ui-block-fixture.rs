//! UI block gallery: every demo spec rendered in a settled transcript turn,
//! folded work header above the reply, the block below it. With
//! `--capture <dir> [--light]` it saves one PNG per spec and exits.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::{path::Path, sync::Arc};
use zuse_protocol::ToolCall;
use zuse_ui::*;

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

fn tool(id: &str, call: ToolCall, output: &str) -> serde_json::Value {
    json!({"id": id, "kind": "tool", "call": call, "isError": false, "resolved": true, "output": output})
}

/// One settled turn: work that folds, a reply, then the block.
fn turn(ix: usize, device: &str, title: &str, spec: &str) -> serde_json::Value {
    let exec = |command: &str| ToolCall::Exec { command: command.into() };
    let read = |path: &str| ToolCall::ReadFile { path: path.into() };
    json!([
        {"id": format!("u{ix}"), "role": "user", "createdAt": 1788900000000_i64, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": format!("Show me: {title}")}]},
        {"id": format!("a{ix}"), "role": "assistant", "createdAt": 1788900001000_i64, "deviceId": device,
         "status": "complete", "durationMs": 1_544_000,
         "parts": [
            {"id": "r0", "kind": "reasoning", "text": "Looking at the workspace first."},
            tool("t1", exec("cargo test -p zuse-ui-block"), "test result: ok. 76 passed"),
            tool("t2", read("crates/ui-block/src/lib.rs"), "//! UI block spec"),
            {"id": "n1", "kind": "text", "text": "Checked the spec crate; rendering the block next."},
            tool("t3", exec("git status --short"), " M crates/ui/src/transcript.rs"),
            tool("t4", ToolCall::Mcp { server: "zuse".into(), tool: "show_ui".into(),
                input: Some(json!({"spec": spec})) }, "{\"ok\":true}"),
            {"id": "p1", "kind": "text", "text": format!("**{title}** · rendered below.")},
            {"id": "ui1", "kind": "uiBlock", "spec": spec},
         ]},
    ])
}

/// A user message carrying a pull request context item, an image and a file
/// mention under a short instruction, after a context-only message.
fn context_chips_turn(device: &str, image: &str) -> serde_json::Value {
    let brief = "Pull request #482 in acme/storefront: Fix checkout total rounding\nhttps://github.com/acme/storefront/pull/482\nBranch: fix/rounding → main\nCheck it out with `gh pr checkout 482 --repo acme/storefront`.\n\n## Inline feedback\n- Unresolved at src/cart/total.ts:88\n  reviewer: Rounding happens before tax is applied; totals drift by a cent.\n- Unresolved at src/cart/total.ts:131\n  reviewer: Please add a test for zero-quantity lines.\n\nMake the requested changes on `fix/rounding`, run the relevant tests, and summarize what changed for each comment. Ask before pushing.";
    let comments = context_items::pull_request_context(
        "https://github.com/acme/storefront/pull/482",
        482,
        context_items::PullRequestScope::Comments,
        brief.into(),
    );
    let checks = context_items::pull_request_context(
        "https://github.com/acme/storefront/pull/482",
        482,
        context_items::PullRequestScope::Checks,
        "These checks are failing:\n- test (ubuntu-latest) (FAILURE)\n\nInspect them with `gh pr checks 482 --repo acme/storefront`.".into(),
    );
    let first = context_items::with_contexts("", &[checks]);
    let second = format!(
        "{}\n\nAttached images (local files — open them to view):\n- {image}",
        context_items::with_contexts(
            &format!(
                "{} Start with [total.ts](zuse-file:src/cart/total.ts).",
                context_items::repair_instruction(482, context_items::PullRequestScope::Comments)
            ),
            &[comments],
        )
    );
    json!([
        {"id": "u0", "role": "user", "createdAt": 1788900000000_i64, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": first}]},
        {"id": "a0", "role": "assistant", "createdAt": 1788900001000_i64, "deviceId": device,
         "status": "complete", "durationMs": 4_000,
         "parts": [{"id": "p", "kind": "text", "text": "The failing job is the rounding test; I'll look at the reviewer comments next."}]},
        {"id": "u1", "role": "user", "createdAt": 1788900002000_i64, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": second}]},
        {"id": "a1", "role": "assistant", "createdAt": 1788900003000_i64, "deviceId": device,
         "status": "complete", "durationMs": 9_000,
         "parts": [{"id": "p", "kind": "text", "text": "Both comments still apply. Moving the rounding after tax and adding the zero-quantity test."}]},
    ])
}

fn click(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, x: f32, y: f32) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        let position = gpui::point(px(x), px(y));
        w.dispatch_event(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
        w.dispatch_event(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position,
                button: gpui::MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        );
        w.dispatch_event(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position,
                button: gpui::MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        );
    })?;
    Ok(())
}

fn capture(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, dir: &Path, name: &str) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        if let Ok(image) = w.render_to_image() {
            image.save(dir.join(format!("{name}.png"))).ok();
        }
    })?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace)?;
    let cwd = workspace.to_string_lossy().into_owned();
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
        "ui-blocks",
        None,
        Some(&core.device_id),
        None,
        Some(cwd.clone()),
    )?;
    core.workspace
        .rename_chat("ui-blocks", "UI blocks")?;
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

    let capture_dir = std::env::args()
        .skip_while(|arg| arg != "--capture")
        .nth(1)
        .map(std::path::PathBuf::from);
    let light = std::env::args().any(|arg| arg == "--light");
    // `--context-chips [--click x,y]`: the context/attachment chips above a
    // user bubble instead of the block gallery; the click opens a pill's card.
    let context_chips = std::env::args().any(|arg| arg == "--context-chips");
    let click_at = std::env::args()
        .skip_while(|arg| arg != "--click")
        .nth(1)
        .and_then(|at| {
            let (x, y) = at.split_once(',')?;
            Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))
        });
    let chip_image = workspace.join("checkout-total-rounding-regression.png");
    image::RgbaImage::from_fn(320, 200, |x, y| {
        image::Rgba([(40 + x / 3) as u8, (90 + y / 2) as u8, 200, 255])
    })
    .save(&chip_image)?;
    let chip_image = chip_image.to_string_lossy().into_owned();
    let specs = zuse_viewport::ui_block::demo_specs();
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
                if light { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark },
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
                s.selected_chat = Some("ui-blocks".into());
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
                            size(px(1100.), px(1000.)),
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
                pause(cx, 800).await;
                if context_chips {
                    let entries = context_chips_turn(&device, &chip_image);
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
                    let Some(dir) = &capture_dir else {
                        return;
                    };
                    std::fs::create_dir_all(dir).ok();
                    pause(cx, 1500).await;
                    capture(window.into(), cx, dir, "context-chips").ok();
                    if let Some((x, y)) = click_at {
                        click(window.into(), cx, x, y).ok();
                        pause(cx, 600).await;
                        capture(window.into(), cx, dir, "context-popover").ok();
                    }
                    cx.update(|cx| cx.quit());
                    return;
                }
                for (ix, (title, spec)) in specs.iter().enumerate() {
                    let entries = turn(ix, &device, title, spec);
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
                    let Some(dir) = &capture_dir else {
                        // Interactive: show the first spec and stay open.
                        return;
                    };
                    std::fs::create_dir_all(dir).ok();
                    pause(cx, 1200).await;
                    let name = format!("{ix:02}-{}", title.to_lowercase().replace(|c: char| !c.is_alphanumeric(), "-"));
                    capture(window.into(), cx, dir, &name).ok();
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
