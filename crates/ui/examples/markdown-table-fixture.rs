//! Native captures for markdown table spacing, alignment, wrapping, and overflow.
use gpui::{
    AppContext, Bounds, Context, IntoElement, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};
use std::{path::PathBuf, time::Duration};
use zuse_ui::{icons, markdown, settings, theme::Theme, typography};

const BODY: &str = r#"The checks are complete. One review is still pending.

| Check | Result |
| --- | --- |
| `checks` (typecheck, formatting, race test, unit tests) | Pass |
| End-to-end, all 4 shards | Pass |
| Code review | Pass, confidence 5/5, ran on this commit with no new comments |
| Security review, deployments, migration label | Pass |
| `human-approval` | Waiting for a reviewer |

All automated review threads are resolved.

| Item | Status | Duration |
| :--- | :---: | ---: |
| **Build** | Ready | 42 s |
| [Preview](https://example.com) | Pending | 3 min |

| Build | Tests | Review | Security | Deploy | Approval |
| --- | --- | --- | --- | --- | --- |
| Passed | Passed | Passed | Passed | Ready | Waiting |
"#;

struct Fixture {
    tree: markdown::BlockTree,
}

impl Render for Fixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .size_full()
            .bg(theme.bg)
            .font_family(theme.font_sans.clone())
            .text_color(theme.text)
            .p(px(24.))
            .child(markdown::render::render_tree(
                &self.tree,
                &markdown::render::RenderOptions::settled("table-fixture".into()),
                theme,
                window,
                &|_| None,
            ))
    }
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("capture directory"));
    std::fs::create_dir_all(&output)?;
    let failure = std::sync::Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_base::init(cx);
            let settings = settings::UiSettings::default();
            let fonts = typography::register_fonts(cx);
            typography::init(
                settings.ui_font_family,
                settings.ui_font_size,
                settings.terminal_font_family,
                settings.terminal_font_size,
                settings.code_font_family,
                settings.code_font_size,
                fonts,
                cx,
            );
            cx.set_global(Theme::dark());
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(30.)),
                            size(px(760.), px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| Fixture {
                            tree: markdown::parse_full(BODY),
                        })
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    for (name, width, theme) in [
                        ("dark", 760., Theme::dark()),
                        ("light", 760., Theme::light()),
                        ("narrow", 380., Theme::dark()),
                    ] {
                        cx.update(|cx| cx.set_global(theme));
                        window.update(cx, |_, w, cx| {
                            w.resize(size(px(width), px(900.)));
                            cx.notify();
                        })?;
                        cx.background_executor()
                            .timer(Duration::from_millis(450))
                            .await;
                        gpui::AnyWindowHandle::from(window).update(
                            cx,
                            |_, w, cx| -> anyhow::Result<()> {
                                w.draw(cx).clear();
                                w.render_to_image()?
                                    .save(output.join(format!("table-{name}.png")))?;
                                Ok(())
                            },
                        )??;
                        if name == "narrow" {
                            gpui::AnyWindowHandle::from(window).update(cx, |_, w, cx| {
                                w.dispatch_event(
                                    gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                                        position: gpui::point(px(200.), px(725.)),
                                        delta: gpui::ScrollDelta::Pixels(gpui::point(
                                            px(-350.),
                                            px(0.),
                                        )),
                                        ..Default::default()
                                    }),
                                    cx,
                                );
                            })?;
                            cx.background_executor()
                                .timer(Duration::from_millis(150))
                                .await;
                            gpui::AnyWindowHandle::from(window).update(
                                cx,
                                |_, w, cx| -> anyhow::Result<()> {
                                    w.draw(cx).clear();
                                    w.render_to_image()?
                                        .save(output.join("table-narrow-scrolled.png"))?;
                                    Ok(())
                                },
                            )??;
                        }
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    *failure.lock().unwrap() = Some(format!("{error:#}"));
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    if let Some(error) = result.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
