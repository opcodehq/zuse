//! A Zapper coordinator chat (or, with ZUSE_FIXTURE_PLAIN, a plain chat) in
//! the production shell whose turn has a Thinking row, a subagent spawn chip
//! and a command group. Isolated data, no agent runs. Prints `ready` once the
//! transcript is on screen, then keeps presenting frames so an outside driver
//! can capture each state. Stdin lines `move X Y` and `click X Y` (logical
//! window pixels) go through the window's own input dispatch, so hover and
//! click take the same path as a real pointer; each prints `ok` once drawn.
//! Close the window to exit.
#[path = "support/fixture_commands.rs"]
mod fixture_commands;

use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::sync::Arc;
use zuse_ui::*;

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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
    let plain = std::env::var_os("ZUSE_FIXTURE_PLAIN").is_some();
    // ZUSE_FIXTURE_DEVIN renders the chat as Devin with an "Agent: Sidekick"
    // chip — the Fusion sidekick shape the pane fix targets.
    let devin = std::env::var_os("ZUSE_FIXTURE_DEVIN").is_some();
    // ZUSE_FIXTURE_ROUTE applies a shell fixture route (e.g.
    // "settings/general") once the transcript is up.
    let route = std::env::var("ZUSE_FIXTURE_ROUTE").ok();
    // ZUSE_FIXTURE_ROOT keeps the engine and UI data dirs under a fixed path
    // instead of a tempdir, and seeds the transcript + subagent docs into the
    // engine's doc host so `zuse serve` on the same engine dir can serve them
    // to the web app (WatchDocMessages reads engine docs, not the UI cache).
    let fixture_root = std::env::var_os("ZUSE_FIXTURE_ROOT").map(std::path::PathBuf::from);
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace)?;
    let engine_dir = fixture_root
        .as_ref()
        .map(|root| root.join("engine"))
        .unwrap_or_else(|| temp.path().join("engine"));
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        // ZUSE_FIXTURE_ROOT mode seeds the LOCAL profile store
        // (`profiles/local`), the same root `zuse serve` uses; the default dev
        // profile (`orgs/dev-org/dev-user`) is invisible to it.
        if fixture_root.is_some() {
            let profile = zuse_engine::EngineProfile::local(&engine_dir)?;
            zuse_engine::EngineCore::assemble_with_profile(
                profile,
                Arc::new(zuse_engine::default_registry()),
                zuse_protocol::HarnessId::ClaudeCode,
                None,
            )
        } else {
            zuse_engine::EngineCore::assemble(
                &engine_dir,
                Arc::new(zuse_engine::default_registry()),
                zuse_protocol::HarnessId::ClaudeCode,
                None,
            )
        }
    })?;
    core.workspace.create_chat(
        "coordinator",
        None,
        Some(&core.device_id),
        None,
        Some(workspace.to_string_lossy().into_owned()),
    )?;
    core.workspace.rename_chat("coordinator", "zappers stuff")?;
    let ipc_port = port();
    let _ipc = runtime.block_on(zuse_engine::serve_ipc(ipc_port, core.rpc_service()))?;
    let data = fixture_root
        .as_ref()
        .map(|root| root.join("ui"))
        .unwrap_or_else(|| temp.path().join("ui"));
    std::fs::create_dir_all(&data)?;
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
    let mut chats = core.workspace.read_chats()?;
    for chat in &mut chats {
        chat.config = Some(
            serde_json::from_value(json!({"harness": if devin { "devin" } else { "claude-code" }, "model": "claude-opus-5-5",
                "reasoning": null, "sandbox": "workspace-write"}))
            .unwrap(),
        );
    }
    let device = core.device_id.clone();
    let zapper: zuse_protocol::zappers::Zapper = serde_json::from_value(json!({
        "id": "zapper", "revision": 1, "status": "active",
        "coordinatorChatId": "coordinator", "previousCoordinatorChats": [],
        "workers": [], "decisions": [],
        "config": {
            "name": "zappers stuff", "purpose": "Ship the Zapper polish", "homeDeviceId": device,
            "repositories": [], "workerDeviceId": device,
            "coordinatorModel": { "harness": "claude-code", "model": null },
            "maxConcurrentWorkers": 4, "maxFollowups": 3
        }
    }))?;
    let now = chrono::Utc::now().timestamp_millis();
    let entries = json!([
        {"id": "u1", "role": "user", "createdAt": now - 150_000, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": "Plan the onboarding issues and research how dusk.bot onboards new users."}]},
        {"id": "a1", "role": "assistant", "createdAt": now - 149_000, "deviceId": device, "status": "complete",
         "parts": [
            {"id": "r1", "kind": "reasoning", "text": "I'm planning new child issues under ZAP-6."},
            {"id": "s1", "kind": "tool", "call": {"kind": "unknown", "name": if devin { "Agent: Sidekick" } else { "Agent: Research dusk.bot onboarding" }, "input": {"description": "Research dusk.bot onboarding"}},
             "isError": false, "resolved": true, "subagentRef": "coordinator--sub--research", "subagentStatus": "done"},
            {"id": "c1", "kind": "tool", "call": {"kind": "exec", "command": "git status"}, "isError": false, "resolved": true},
            {"id": "c2", "kind": "tool", "call": {"kind": "exec", "command": "git log --oneline -5"}, "isError": false, "resolved": true},
            {"id": "p1", "kind": "text", "text": "Short update: I've created issues for your new asks: ZAP-9 for the creation screen and ZAP-10 for the self-written purpose."},
         ]},
    ]);
    let subagent = json!([
        {"id": "su", "role": "user", "createdAt": now - 148_000, "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text": "Research how dusk.bot onboards new users and summarize the steps."}]},
        {"id": "sa", "role": "assistant", "createdAt": now - 120_000, "deviceId": device, "status": "complete",
         "parts": [
            {"id": "x1", "kind": "tool", "call": {"kind": "exec", "command": "curl -s https://example.test/onboarding"}, "isError": false, "resolved": true},
            {"id": "x2", "kind": "text", "text": "Onboarding is three steps: pick a workspace, connect a repository, then send a first message from a short guide."},
         ]},
    ]);

    // ZUSE_FIXTURE_ROOT mode: mirror the seeded transcript into the engine's
    // doc host so a `zuse serve` on the same engine dir can serve it to the
    // web app. The UI reads its own cache; the web app reads engine docs.
    if fixture_root.is_some() {
        runtime.block_on(async {
            let chat_entries: Vec<zuse_doc::SessionMessageEntry> =
                serde_json::from_value(entries.clone()).expect("seed chat entries decode");
            let sub_entries: Vec<zuse_doc::SessionMessageEntry> =
                serde_json::from_value(subagent.clone()).expect("seed subagent entries decode");
            let chat_doc = core
                .doc_host
                .open("coordinator")
                .expect("open coordinator doc");
            for entry in &chat_entries {
                chat_doc.doc().push_message(entry).expect("push chat entry");
            }
            let sub_doc = core
                .doc_host
                .open("coordinator--sub--research")
                .expect("open subagent doc");
            for entry in &sub_entries {
                sub_doc
                    .doc()
                    .push_message(entry)
                    .expect("push subagent entry");
            }
            core.doc_host.flush_all();
            // The chat list lives in the workspace registry snapshot, not the
            // doc store — flush it too or `zuse serve` sees zero chats.
            core.workspace.flush();
        });
        eprintln!("[fixture] seeded engine docs for coordinator + subagent");
        // ZUSE_FIXTURE_SEED_ONLY exits before the GUI app starts: the seeded
        // docs are already flushed to the engine store, so `zuse serve` on the
        // same data dir can serve them without a window to keep alive.
        if std::env::var_os("ZUSE_FIXTURE_SEED_ONLY").is_some() {
            return Ok(());
        }
    }

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
                if !plain {
                    s.zappers = vec![zapper];
                }
                s.selected_chat = Some("coordinator".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.no_project = true;
                s
            });
            let shell_slot =
                std::rc::Rc::new(std::cell::RefCell::new(None::<gpui::Entity<shell::Shell>>));
            let slot = shell_slot.clone();
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(0.), px(0.)),
                            size(px(1280.), px(800.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        let shell = cx.new(|cx| shell::Shell::new(state.clone(), boot, cx));
                        *slot.borrow_mut() = Some(shell.clone());
                        shell
                    },
                )
                .unwrap();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            state.update(cx, |_, cx| cx.notify());
            // No activate: the window must not take keyboard focus from the
            // desktop it opens on. Driven input arrives on stdin.
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
                // The subagent's own transcript, as a finished run leaves it.
                // This engine holds no blob for it, so the chip's open falls
                // back to a doc watch; seeding again after it keeps the rows.
                let subagent: Vec<zuse_doc::SessionMessageEntry> =
                    serde_json::from_value(subagent).unwrap();
                let seed = |cx: &mut gpui::AsyncApp| {
                    state.update(cx, |s, cx| {
                        s.set_subagent_snapshot(
                            "coordinator--sub--research".into(),
                            subagent.clone(),
                        );
                        cx.notify();
                    })
                };
                seed(cx);
                pause(1200).await;
                if let Some(route) = &route {
                    if let Some(shell) = shell_slot.borrow().clone() {
                        shell.update(cx, |s, cx| {
                            s.fixture_review_route(route, cx);
                        });
                    }
                    pause(600).await;
                }
                present(window.into(), cx);
                println!("ready");
                let (send, commands) = std::sync::mpsc::channel::<String>();
                std::thread::spawn(move || {
                    for line in std::io::stdin().lines().map_while(Result::ok) {
                        if send.send(line).is_err() {
                            break;
                        }
                    }
                });
                let mut tick = 0u64;
                loop {
                    pause(100).await;
                    if tick.is_multiple_of(10) {
                        seed(cx);
                    }
                    tick += 1;
                    while let Ok(line) = commands.try_recv() {
                        // `capture PATH` renders the window to a PNG — a real
                        // capture of the compiled fixture, no screen-recording
                        // permission needed.
                        if let Some(path) = fixture_commands::capture_path(&line) {
                            let path = path.to_string();
                            let saved = gpui::AnyWindowHandle::from(window).update(
                                cx,
                                |_, w, _| -> anyhow::Result<()> {
                                    Ok(w.render_to_image()?.save(path.as_str())?)
                                },
                            );
                            match saved {
                                Ok(Ok(())) => println!("ok"),
                                Ok(Err(e)) | Err(e) => println!("capture failed: {e:#}"),
                            }
                            continue;
                        }
                        let words: Vec<&str> = line.split_whitespace().collect();
                        // `scroll X Y DY` dispatches a wheel event so long
                        // pages (settings) can be brought to a row.
                        if words.first() == Some(&"scroll") && words.len() == 4 {
                            let (Ok(x), Ok(y), Ok(dy)) = (
                                words[1].parse::<f32>(),
                                words[2].parse::<f32>(),
                                words[3].parse::<f32>(),
                            ) else {
                                continue;
                            };
                            gpui::AnyWindowHandle::from(window)
                                .update(cx, |_, window, cx| {
                                    window.dispatch_event(
                                        gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                                            position: gpui::point(px(x), px(y)),
                                            delta: gpui::ScrollDelta::Pixels(gpui::point(
                                                px(0.),
                                                px(dy),
                                            )),
                                            ..Default::default()
                                        }),
                                        cx,
                                    );
                                })
                                .ok();
                            present(window.into(), cx);
                            println!("ok");
                            continue;
                        }
                        let [verb, x, y] = words[..] else { continue };
                        let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else {
                            continue;
                        };
                        let position = gpui::point(px(x), px(y));
                        let mut events =
                            vec![gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                position,
                                pressed_button: None,
                                modifiers: Default::default(),
                            })];
                        if verb == "click" {
                            events.push(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                                button: gpui::MouseButton::Left,
                                position,
                                modifiers: Default::default(),
                                click_count: 1,
                                first_mouse: false,
                            }));
                            events.push(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                                button: gpui::MouseButton::Left,
                                position,
                                modifiers: Default::default(),
                                click_count: 1,
                            }));
                        }
                        for event in events {
                            gpui::AnyWindowHandle::from(window)
                                .update(cx, |_, window, cx| {
                                    window.dispatch_event(event, cx);
                                })
                                .ok();
                            present(window.into(), cx);
                        }
                        println!("ok");
                    }
                    present(window.into(), cx);
                }
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    Ok(())
}
