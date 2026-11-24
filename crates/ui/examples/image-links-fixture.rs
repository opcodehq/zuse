//! Screenshots of image links in a cloud chat, through the real shell:
//! the reply's image link (photo icon + link text), the tab that opens at
//! once while the sleeping computer wakes, the inline error with a retry,
//! and the image once the computer is up. Scripted data only.
//!
//! `cargo run -p zuse-ui --example image-links-fixture --features
//! image-links-fixture -- <output-dir>` (on Linux, under `xvfb-run`).
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use image::{Rgba, RgbaImage};
use std::{path::Path, path::PathBuf, time::Duration};
use zuse_ui::*;

const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 800.0;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        window.draw(cx).clear();
        window.present_if_needed();
        match window.render_to_image() {
            Ok(image) => image.save(output.join(name))?,
            // Linux has no offscreen readback: grab the fixture window's
            // area of the private virtual display (run under xvfb-run).
            Err(_) => {
                let status = std::process::Command::new("import")
                    .args([
                        "-window",
                        "root",
                        "-crop",
                        &format!("{}x{}+0+0", WIDTH as u32, HEIGHT as u32),
                        "+repage",
                    ])
                    .arg(output.join(name))
                    .status()?;
                anyhow::ensure!(status.success(), "X11 capture failed");
            }
        }
        Ok(())
    })?
}

/// A stand-in screenshot of the reasoning slider the reply links to.
fn slider_png() -> Vec<u8> {
    let (w, h) = (960u32, 520u32);
    let mut image = RgbaImage::from_pixel(w, h, Rgba([22, 23, 27, 255]));
    let card = |x: u32, y: u32| x > 80 && x < w - 80 && y > 120 && y < h - 120;
    for y in 0..h {
        for x in 0..w {
            if card(x, y) {
                image.put_pixel(x, y, Rgba([34, 36, 42, 255]));
            }
        }
    }
    let (left, right, track_y) = (160u32, 800u32, 260u32);
    let knob = 600u32;
    for x in left..right {
        for y in track_y - 3..track_y + 3 {
            let color = if x < knob {
                Rgba([151, 183, 76, 255])
            } else {
                Rgba([70, 72, 80, 255])
            };
            image.put_pixel(x, y, color);
        }
    }
    for step in 0..5u32 {
        let x = left + step * (right - left) / 4;
        for y in track_y + 18..track_y + 30 {
            for dx in 0..3 {
                image.put_pixel(x + dx - 1, y, Rgba([120, 122, 130, 255]));
            }
        }
    }
    for y in track_y - 18..track_y + 18 {
        for x in knob - 18..knob + 18 {
            let (dx, dy) = (x as f32 - knob as f32, y as f32 - track_y as f32);
            if dx * dx + dy * dy <= 18.0 * 18.0 {
                image.put_pixel(x, y, Rgba([236, 236, 240, 255]));
            }
        }
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode png");
    bytes.into_inner()
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    // The scenario's outcome, read once the application loop returns.
    let passed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let scenario_passed = passed.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let prefs = settings::UiSettings::default();
            settings::init(prefs.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                prefs.ui_font_family.clone(),
                prefs.ui_font_size,
                prefs.terminal_font_family.clone(),
                prefs.terminal_font_size,
                prefs.code_font_family.clone(),
                prefs.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data.clone(), cx);
            appearance::init(
                appearance::AppearanceMode::Dark,
                prefs.theme_selection,
                prefs.accent,
                prefs.surface,
                cx,
            );
            history::init(
                prefs.git_history_columns,
                prefs.git_history_column_widths,
                prefs.git_history_column_order,
                prefs.git_history_author_display,
                cx,
            );
            composer::init(cx, prefs.composer_send_behavior);
            terminal::panel::init(cx);
            app_menus::init(cx);
            let boot = EngineBootConfig {
                target: None,
                data_dir: data.join("ui"),
                ipc_port: 0,
                edge_url: "http://127.0.0.1:1".into(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: zuse_protocol::HarnessId::Mock,
            };
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(0.0), px(0.0)),
                            size(px(WIDTH), px(HEIGHT)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|cx| {
                            let state = cx.new(|_| {
                                let mut s = state::AppState::new();
                                s.connection = zuse_protocol::view::ConnectionStatus::Ready;
                                s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
                                s.local_device_id = Some("fixture-local".into());
                                s.auto_selected = true;
                                s.spaces_synced = true;
                                s.chats_synced = true;
                                s
                            });
                            let mut shell = shell::Shell::new(state, boot, cx);
                            shell.fixture_open_sleeping_cloud_chat(cx);
                            shell
                        })
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    let any: gpui::AnyWindowHandle = window.into();
                    pause(cx, 1800).await;
                    capture(any, cx, &output, "a-reply-image-link.png")?;

                    window.update(cx, |shell, window, cx| {
                        shell.fixture_click_image_link(window, cx)
                    })?;
                    pause(cx, 900).await;
                    capture(any, cx, &output, "b-waking-tab.png")?;

                    window.update(cx, |shell, _, cx| shell.fixture_fail_wake(cx))?;
                    pause(cx, 500).await;
                    capture(any, cx, &output, "d-wake-failed-retry.png")?;

                    window.update(cx, |shell, _, cx| {
                        shell.fixture_wake_with_image(slider_png(), cx)
                    })?;
                    pause(cx, 2000).await;
                    capture(any, cx, &output, "c-image-loaded.png")?;
                    Ok(())
                }
                .await;
                if let Err(error) = &result {
                    eprintln!("image links fixture failed: {error:?}");
                }
                scenario_passed.store(result.is_ok(), std::sync::atomic::Ordering::SeqCst);
                std::fs::write(
                    output.join("result.txt"),
                    if result.is_ok() { "PASS\n" } else { "FAIL\n" },
                )
                .ok();
                let windows = cx.update(|cx| cx.windows());
                for handle in windows {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    anyhow::ensure!(
        passed.load(std::sync::atomic::Ordering::SeqCst),
        "image links fixture failed"
    );
    Ok(())
}
