//! Native visual evidence for the ported shortcut/composer surfaces, with offline data.
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use zuse_ui::*;

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(450))
        .await;
}
async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    path: &Path,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.refresh();
        w.draw(cx).clear();
        w.present_if_needed();
    })?;
    pause(cx).await;
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/examples/private-secrets-fixture/capture.py"
            ))
            .arg(std::process::id().to_string())
            .arg(path)
            .status()?;
        anyhow::ensure!(status.success(), "fixture window capture failed");
    }
    #[cfg(not(target_os = "linux"))]
    window.update(cx, |_, w, _| w.render_to_image()?.save(path))??;
    Ok(())
}
fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("capture directory"));
    std::fs::create_dir_all(&output)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let failure = std::sync::Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let settings = settings::UiSettings::default();
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        appearance::init(appearance::AppearanceMode::Dark, settings.theme_selection, settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths, settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior); terminal::panel::init(cx); app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zuse_protocol::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
            s.local_device_id = Some("local".into());
            s.selected_chat = Some("browser-fixture".into()); s.selected_space = Some("project".into());
            s.auto_selected = true; s.chats_synced = true; s.spaces_synced = true;
            s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"project","deviceId":"local","path":"/tmp/fieldnotes","createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
            s.chats = vec![serde_json::from_value(serde_json::json!({"id":"browser-fixture","deviceId":"local","spaceId":"project","title":"Review the workspace","archived":false,"createdAt":"2026-09-08T00:00:00Z","config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap()];
            s
        });
        let boot = EngineBootConfig { target: None, data_dir: data, ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(12.),px(30.)), size(px(1100.),px(850.))))),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                pause(cx).await;
                window.update(cx, |s, _, cx| s.fixture_review_route("main:transcript", cx))?;
                pause(cx).await;
                for (mode, name) in [(appearance::AppearanceMode::Dark,"dark"), (appearance::AppearanceMode::Light,"light")] {
                    cx.update(|cx| appearance::set_mode(mode,cx));
                    for kind in ["find", "find-empty", "stash", "history", "emoji", "stash-empty", "notice", "shortcuts"] {
                        window.update(cx, |s,w,cx| s.fixture_shortcut_visual(kind,w,cx))?;
                        pause(cx).await;
                        capture(window.into(),cx,&output.join(format!("{kind}-{name}.png"))).await?;
                        if kind == "stash" {
                            // The fixed 1100×850 capture was inspected before choosing this
                            // position in the first saved draft's trailing action slot.
                            window.update(cx, |_, w, cx| { w.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position: gpui::point(px(1008.), px(638.)), ..Default::default() }), cx); })?;
                            pause(cx).await;
                            capture(window.into(),cx,&output.join(format!("stash-hover-{name}.png"))).await?;
                        }
                    }
                }
                window.update(cx, |s,w,cx| { w.resize(size(px(600.),px(850.))); s.fixture_shortcut_visual("shortcuts",w,cx); })?;
                pause(cx).await;
                capture(window.into(),cx,&output.join("shortcuts-narrow.png")).await?;
                Ok(())
            }.await;
            if let Err(error) = run { *failure.lock().unwrap() = Some(format!("{error:#}")); }
            window.update(cx, |_, w, _| w.remove_window()).ok();
            pause(cx).await;
            cx.update(|cx| cx.quit());
        }).detach();
    });
    if let Some(error) = result.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
