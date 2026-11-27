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
    let exec = |command: &str| ToolCall::Exec {
        command: command.into(),
    };
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

const SETUP_PROPOSAL: &str = "msp_0f5c2a9e7d3b41c8a6e2f19b4d7c0a35";
const SETUP_BEFORE: &str =
    "set -euo pipefail\nsudo apt-get update\nsudo apt-get install -y ripgrep jq\nnpm i -g pnpm\n";
const SETUP_AFTER: &str = "set -euo pipefail\nsudo apt-get update\nsudo apt-get install -y ripgrep jq build-essential pkg-config libssl-dev\nnpm i -g pnpm\n# Rust for the API service\ncommand -v cargo || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal\necho 'export PATH=\"$HOME/.cargo/bin:$PATH\"' | sudo tee /etc/profile.d/cargo.sh\n# Bun for the web app\ncommand -v bun || curl -fsSL https://bun.sh/install | bash\n";

/// A setup chat's turn: the agent reads the image configuration, tests a
/// script on its computer and proposes the change as a live card.
fn machine_setup_turn(device: &str) -> serde_json::Value {
    let mcp = |tool: &str, input: serde_json::Value| ToolCall::Mcp {
        server: "zuse".into(),
        tool: tool.into(),
        input: Some(input),
    };
    let spec = format!("root = MachineImageProposal(\"{SETUP_PROPOSAL}\")");
    json!([
        {"id": "u1", "role": "user", "createdAt": 1788900000000_i64, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": "Install Rust and bun, and add acme/api to my image."}]},
        {"id": "a1", "role": "assistant", "createdAt": 1788900001000_i64, "deviceId": device,
         "status": "complete", "durationMs": 214_000,
         "parts": [
            tool("t1", mcp("get_machine_image_config", json!({})), "{\"revision\":4}"),
            tool("t2", mcp("test_setup_script", json!({"setup_script": SETUP_AFTER})), "{\"succeeded\":true}"),
            tool("t3", mcp("propose_machine_image_config", json!({"setup_script": SETUP_AFTER, "repositories": ["github.com/acme/web", "github.com/acme/api"]})), "{\"ok\":true}"),
            {"id": "p1", "kind": "text", "text": "The script ran cleanly here in 3 min 12 s. It adds Rust and bun, and acme/api joins your image."},
            {"id": "ui1", "kind": "uiBlock", "spec": spec},
         ]},
    ])
}

/// The proposal the setup card shows: `proposed`, `building` or `ready`.
fn machine_setup_view(stage: &str) -> zuse_protocol::machine_setup::ProposalView {
    let image = match stage {
        "building" => {
            json!({"state":"building","phase":"setup","reason":"manual","recipeRevision":5,"updatedAt":2})
        }
        "ready" => {
            json!({"state":"ready","reason":"manual","recipeRevision":5,"builtAt":1788900400000_i64,"updatedAt":3,
            "repositories":[{"repositoryIdentity":"github.com/acme/web","state":"baked","commit":"4be1c2d"},
                            {"repositoryIdentity":"github.com/acme/api","state":"baked","commit":"9ad03f1"}]})
        }
        _ => {
            json!({"state":"ready","reason":"recipe","recipeRevision":4,"builtAt":1788800000000_i64,"updatedAt":1})
        }
    };
    let image_state = if stage == "building" {
        "building"
    } else {
        "ready"
    };
    serde_json::from_value(json!({
        "proposal": {
            "proposalId": SETUP_PROPOSAL, "chatId": "ui-blocks", "baseRevision": 4,
            "currentSetupScript": SETUP_BEFORE, "setupScript": SETUP_AFTER,
            "currentRepositories": ["github.com/acme/web"],
            "repositories": ["github.com/acme/web", "github.com/acme/api"],
            "notes": "Adds Rust (rustup, minimal profile) and bun, with the build tools Rust crates need, and bakes acme/api.",
            "state": if stage == "proposed" { "proposed" } else { "applied" },
            "createdAt": 1788900001000_i64,
        },
        "machine": {"revision": if stage == "proposed" { 4 } else { 5 }, "recipe": {"setupScript": SETUP_BEFORE},
            "createdAt": 1, "imageState": image_state, "image": image},
    }))
    .unwrap()
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

fn click(
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
    x: f32,
    y: f32,
) -> anyhow::Result<()> {
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

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
    dir: &Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.present_if_needed();
        let path = dir.join(format!("{name}.png"));
        match w.render_to_image() {
            Ok(image) => {
                image.save(&path).ok();
            }
            // Without offscreen rendering (e.g. Xvfb), read the X screen.
            Err(_) => {
                std::process::Command::new("import")
                    .args(["-window", "root"])
                    .arg(&path)
                    .status()
                    .ok();
            }
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
    // Debug builds of the engine's futures outgrow tokio's default 2 MiB
    // worker stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()?;
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
    core.workspace.rename_chat("ui-blocks", "UI blocks")?;
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
    // `--plugins <signedin|empty|signedout>`: seed the plugin state live
    // plugin cards read. `--only <text>`: just the specs whose title has it.
    let plugins = std::env::args().skip_while(|arg| arg != "--plugins").nth(1);
    let only = std::env::args().skip_while(|arg| arg != "--only").nth(1);
    // `--context-chips [--click x,y]`: the context/attachment chips above a
    // user bubble instead of the block gallery; the click opens a pill's card.
    let context_chips = std::env::args().any(|arg| arg == "--context-chips");
    // `--machine-setup <proposed|building|ready>`: a setup chat's proposal card.
    let machine_setup = std::env::args()
        .skip_while(|arg| arg != "--machine-setup")
        .nth(1);
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
                if light {
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
                        // Opaque for the setup card, which is read off an X screen.
                        window_background: if machine_setup.is_some() {
                            gpui::WindowBackgroundAppearance::Opaque
                        } else {
                            theme::Theme::of(cx).window_background_appearance()
                        },
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
            if let Some(variant) = &plugins {
                settings::plugins::fixture_seed(state.clone(), variant, cx);
            }
            state.update(cx, |_, cx| cx.notify());
            cx.activate(true);
            cx.spawn(async move |cx| {
                pause(cx, 800).await;
                if let Some(stage) = &machine_setup {
                    let view = machine_setup_view(stage);
                    cx.update(|cx| {
                        machine_setup_cards::MachineSetupCards::fixture_seed(
                            state.clone(),
                            view,
                            cx,
                        )
                    });
                    let entries = machine_setup_turn(&device);
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
                    capture(window.into(), cx, dir, &format!("setup-chat-{stage}")).ok();
                    cx.update(|cx| cx.quit());
                    return;
                }
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
                    if only
                        .as_ref()
                        .is_some_and(|only| !title.contains(only.as_str()))
                    {
                        continue;
                    }
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
                    let name = format!(
                        "{ix:02}-{}",
                        title
                            .to_lowercase()
                            .replace(|c: char| !c.is_alphanumeric(), "-")
                    );
                    capture(window.into(), cx, dir, &name).ok();
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
