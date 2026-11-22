//! Native evidence for Zapper avatars in the real shell: the sidebar, the
//! Zapper panel header and the settings picker, with a mascot and with an
//! uploaded picture. Offline data only; no engine runs and nothing is sent.
//! Usage: `zapper-avatar-fixture <capture directory>`.
use gpui::{AnyWindowHandle, AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::Path, time::Duration};
use zuse_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

async fn capture(window: AnyWindowHandle, cx: &mut AsyncApp, path: &Path) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.refresh();
        w.draw(cx).clear();
        w.present_if_needed();
    })?;
    pause(cx, 450).await;
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

/// A wide sunset, cropped and shrunk exactly as an upload would be.
fn sample_photo() -> String {
    let (w, h) = (1200u32, 800u32);
    let image = image::RgbaImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let sun = ((fx - 0.5).powi(2) + (fy - 0.55).powi(2)).sqrt() < 0.16;
        let hill = fy > 0.72 + 0.06 * (fx * 9.0).sin();
        image::Rgba(if hill {
            [40, 70 + (fy * 40.) as u8, 60, 255]
        } else if sun {
            [255, 214, 120, 255]
        } else {
            [
                (250. - fy * 120.) as u8,
                (120. + fy * 40.) as u8,
                (110. + fy * 110.) as u8,
                255,
            ]
        })
    });
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    zuse_viewport::zapper_avatar::prepare_image(&bytes).unwrap()
}

fn zapper(
    id: &str,
    name: &str,
    mascot: &str,
    color: &str,
    image: Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id, "revision": 3, "status": "active",
        "coordinatorChatId": format!("zot-{id}-0"), "previousCoordinatorChats": [],
        "workers": [], "decisions": [], "issues": [],
        "config": {
            "name": name, "purpose": "Keep staging healthy and ship the release.",
            "homeDeviceId": "local", "repositories": [{"deviceId": "local", "spaceId": "project"}],
            "workerDeviceId": "local",
            "coordinatorModel": {"harness": "claude-code", "model": null},
            "maxConcurrentWorkers": 4, "maxFollowups": 3,
            "avatar": {"mascot": mascot, "color": color},
            "avatarImage": image
        }
    })
}

fn main() -> anyhow::Result<()> {
    let output = std::path::PathBuf::from(std::env::args().nth(1).expect("capture directory"));
    std::fs::create_dir_all(&output)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let settings = settings::UiSettings::default();
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
        let photo = sample_photo();
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zuse_protocol::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
            s.local_device_id = Some("local".into());
            s.devices = serde_json::from_value(serde_json::json!([
                {"id":"local","name":"Studio","platform":"linux","lastSeenAt":null}
            ]))
            .unwrap();
            s.auto_selected = true;
            s.chats_synced = true;
            s.spaces_synced = true;
            s.selected_space = Some("project".into());
            s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"project","deviceId":"local","path":"/tmp/fieldnotes","createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
            let zots = [
                zapper("release", "Release", "hex", "teal", None),
                zapper("photo", "Photo Zapper", "bolt", "coral", Some(photo.clone())),
                zapper("docs", "Docs", "squire", "violet", None),
                zapper("triage", "Triage", "brawler", "yellow", None),
                zapper("night", "Night shift", "viking", "blue", None),
            ];
            s.zots = zots
                .iter()
                .map(|z| serde_json::from_value(z.clone()).unwrap())
                .collect();
            zuse_viewport::zapper_avatar::remember_zots(&s.zots);
            s.chats = zots
                .iter()
                .map(|z| {
                    serde_json::from_value(serde_json::json!({
                        "id": z["coordinatorChatId"], "deviceId": "local", "spaceId": "project",
                        "title": z["config"]["name"], "archived": false,
                        "createdAt": "2026-09-08T00:00:00Z",
                        "config": {"harness":"claude-code","model":null,"reasoning":null,"sandbox":"workspace-write"}
                    }))
                    .unwrap()
                })
                .collect();
            s.selected_chat = Some("zot-release-0".into());
            s
        });
        let boot = EngineBootConfig {
            target: None,
            data_dir: data,
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
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        gpui::point(px(12.), px(30.)),
                        size(px(1280.), px(820.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
            )
            .unwrap();
        cx.activate(true);
        let select = move |chat: &'static str, cx: &mut gpui::App| {
            state.update(cx, |s, cx| {
                s.selected_chat = Some(chat.into());
                cx.notify();
            })
        };
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                pause(cx, 1200).await;
                capture(window.into(), cx, &output.join("shell-mascot.png")).await?;
                window.update(cx, |_, _, cx| select("zot-photo-0", cx))?;
                pause(cx, 300).await;
                capture(window.into(), cx, &output.join("shell-image.png")).await?;
                window.update(cx, |_, _, cx| select("zot-release-0", cx))?;
                window.update(cx, |shell, _, cx| shell.fixture_open_zot_editor("release", cx))?;
                pause(cx, 300).await;
                capture(window.into(), cx, &output.join("picker-mascot.png")).await?;
                window.update(cx, |_, _, cx| select("zot-photo-0", cx))?;
                window.update(cx, |shell, _, cx| shell.fixture_open_zot_editor("photo", cx))?;
                pause(cx, 300).await;
                capture(window.into(), cx, &output.join("picker-image.png")).await?;
                Ok(())
            }
            .await;
            if let Err(error) = run {
                eprintln!("fixture failed: {error:#}");
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(())
}
