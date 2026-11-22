//! Isolated visual and keyboard evidence for the composer's model picker
//! (provider rail, favorites, detail card) and its access control.
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use std::{path::Path, path::PathBuf, time::Duration};
use zuse_ui::*;

struct Fixture {
    composer: Entity<composer::Composer>,
    title: &'static str,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::Theme::of(cx).clone();
        div()
            .id("model-picker-fixture")
            .size_full()
            .bg(theme.surface)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .p(px(28.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(settings::widgets::page_header(&theme, self.title, None))
            .child(div().flex_1())
            .child(self.composer.clone())
    }
}

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(320))
        .await;
}

async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &Path,
    name: &str,
) -> anyhow::Result<()> {
    pause(cx).await;
    window.update(cx, |_, window, cx| {
        window.draw(cx).clear();
        window.present_if_needed();
        match window.render_to_image() {
            Ok(image) => image.save(output.join(name))?,
            // Linux has no offscreen readback: grab the fixture window's
            // area of the private virtual display (run under xvfb-run).
            Err(_) => {
                let status = std::process::Command::new("import")
                    .args(["-window", "root", "-crop", "1180x760+0+0", "+repage"])
                    .arg(output.join(name))
                    .status()?;
                anyhow::ensure!(status.success(), "X11 capture failed");
            }
        }
        Ok(())
    })?
}

fn press(window: gpui::AnyWindowHandle, cx: &mut AsyncApp, key: &str) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        assert!(
            window.dispatch_keystroke(gpui::Keystroke::parse(key).unwrap(), cx),
            "model picker did not handle {key}"
        );
    })?;
    Ok(())
}

async fn scene(
    window: gpui::WindowHandle<Fixture>,
    cx: &mut AsyncApp,
    output: &Path,
    appearance: appearance::AppearanceMode,
    label: &str,
) -> anyhow::Result<()> {
    window.update(cx, |view, window, cx| {
        appearance::set_mode(appearance, cx);
        view.title = if appearance == appearance::AppearanceMode::Light {
            "Model picker · light"
        } else {
            "Model picker · dark"
        };
        view.composer.update(cx, |composer, cx| {
            composer
                .pickers()
                .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
        });
        cx.notify();
    })?;
    pause(cx).await;
    pause(cx).await;
    capture(
        window.into(),
        cx,
        output,
        &format!("model-picker-{label}.png"),
    )
    .await?;

    // The card follows the highlighted row: a model with fast mode and a
    // context window choice.
    window.update(cx, |view, _, cx| {
        view.composer.update(cx, |composer, cx| {
            composer.pickers().update(cx, |pickers, cx| {
                pickers.fixture_highlight_model("opus", cx)
            });
        });
    })?;
    pause(cx).await;
    capture(
        window.into(),
        cx,
        output,
        &format!("model-picker-detail-{label}.png"),
    )
    .await?;
    press(window.into(), cx, "escape")?;
    pause(cx).await;

    window.update(cx, |view, _, cx| {
        view.composer
            .update(cx, |composer, cx| composer.fixture_open_permissions(cx));
    })?;
    pause(cx).await;
    capture(
        window.into(),
        cx,
        output,
        &format!("access-menu-{label}.png"),
    )
    .await?;
    window.update(cx, |view, _, cx| {
        view.composer
            .update(cx, |composer, cx| composer.fixture_close_permissions(cx));
    })?;
    pause(cx).await;
    capture(window.into(), cx, output, &format!("composer-{label}.png")).await?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut prefs = settings::UiSettings::default();
            prefs.compact_model_picker = true;
            prefs.default_model = Some(zuse_protocol::presentation::DefaultModelSelection {
                provider: zuse_protocol::HarnessId::Codex,
                model: "gpt-5.4".into(),
                label: "GPT-5.4".into(),
            });
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
            theme_library::init(data, cx);
            appearance::init(
                appearance::AppearanceMode::Dark,
                prefs.theme_selection,
                prefs.accent,
                prefs.surface,
                cx,
            );
            composer::init(cx, prefs.composer_send_behavior);

            let state = cx.new(|_| {
                let mut state = state::AppState::new();
                state.connection = zuse_protocol::view::ConnectionStatus::Ready;
                state.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
                state.local_device_id = Some("fixture-device".into());
                state
            });
            let composer = cx.new(|cx| composer::Composer::new(state, cx));
            composer.update(cx, |composer, cx| {
                composer
                    .pickers()
                    .update(cx, |pickers, cx| pickers.fixture_compact_catalog(cx));
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(0.0), px(0.0)),
                            size(px(1180.0), px(760.0)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| Fixture {
                            composer,
                            title: "Model picker · dark",
                        })
                    },
                )
                .unwrap();
            cx.activate(true);

            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    pause(cx).await;
                    scene(window, cx, &output, appearance::AppearanceMode::Dark, "dark").await?;
                    scene(window, cx, &output, appearance::AppearanceMode::Light, "light")
                        .await?;

                    // Keyboard: move to a row, step its reasoning from the
                    // card, then pick it with Enter.
                    window.update(cx, |view, window, cx| {
                        appearance::set_mode(appearance::AppearanceMode::Dark, cx);
                        view.title = "Model picker · keyboard";
                        view.composer.update(cx, |composer, cx| {
                            composer
                                .pickers()
                                .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
                        });
                    })?;
                    pause(cx).await;
                    for key in ["up", "up", "left"] {
                        press(window.into(), cx, key)?;
                    }
                    pause(cx).await;
                    capture(window.into(), cx, &output, "model-picker-keyboard-dark.png").await?;
                    press(window.into(), cx, "enter")?;
                    pause(cx).await;
                    capture(window.into(), cx, &output, "composer-after-pick-dark.png").await?;

                    std::fs::write(
                        output.join("result.txt"),
                        "PASS: model picker with rail, favorites and detail card; access menu; dark and light.\n",
                    )?;
                    Ok(())
                }
                .await;
                if let Err(error) = result {
                    eprintln!("model picker fixture failed: {error:#}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
