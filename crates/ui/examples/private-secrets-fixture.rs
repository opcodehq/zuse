//! Screenshots of the production secret card using synthetic data, with no network operation.
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use std::{path::PathBuf, time::Duration};
use zuse_ui::*;
struct Fixture {
    composer: Entity<composer::Composer>,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::Theme::of(cx).clone();
        div()
            .size_full()
            .bg(t.surface)
            .text_color(t.text)
            .font_family(t.font_sans.clone())
            .flex()
            .flex_col()
            .p(px(28.))
            .gap(px(18.))
            .child(
                div()
                    .text_size(px(21.))
                    .child("Question and answer composer"),
            )
            .child(
                div()
                    .text_color(t.text_muted)
                    .child("Production desktop UI · synthetic demo data"),
            )
            .child(div().flex_1())
            .child(self.composer.clone())
    }
}
async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(700))
        .await;
}
fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let prefs = settings::UiSettings::default();
        settings::init(prefs.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(prefs.ui_font_family.clone(),prefs.ui_font_size,prefs.terminal_font_family.clone(),prefs.terminal_font_size,prefs.code_font_family.clone(),prefs.code_font_size,fonts,cx);
        theme_library::init(data.clone(),cx);
        appearance::init(appearance::AppearanceMode::Dark,prefs.theme_selection,prefs.accent,prefs.surface,cx);
        composer::init(cx,prefs.composer_send_behavior);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zuse_protocol::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
            s.selected_chat = Some("secret-fixture".into());
            s.local_device_id = Some("fixture-device".into());
            s.chats = serde_json::from_value(serde_json::json!([{"id":"secret-fixture","deviceId":"fixture-device","spaceId":null,"title":"Check GitHub API access","archived":false,"createdAt":"2026-10-29T00:00:00Z","config":{"harness":"claude-code","sandbox":"workspace-write"}}])).unwrap();
            s.transcript = serde_json::from_value(serde_json::json!([{"id":"assistant","role":"assistant","parts":[{"kind":"input","id":"secret-card","requestId":"visual-request","questions":[{"id":"private-secret:visual-demo","header":"Private secret","question":"GitHub API token\nCheck account access for this operation.\n\nApprove one request: GET https://api.github.com/user\nBody: \n\nBearer token is used once by the backend. Only HTTP status is returned. Expires in 10 minutes. No file or shell access.","options":[],"multiSelect":false,"multiline":false}],"resolved":false}],"createdAt":0,"deviceId":"fixture-device","status":"streaming"}])).unwrap();
            s
        });
        let composer = cx.new(|cx| composer::Composer::new(state.clone(),cx));
        let window = cx.open_window(WindowOptions {window_bounds:Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(0.),px(0.)),size(px(1080.),px(620.))))),..Default::default()},|_,cx|cx.new(|_|Fixture{composer})).unwrap();
        state.update(cx,|_,cx|cx.notify()); cx.activate(true);
        cx.spawn(async move |cx| {
            pause(cx).await;
            for (name, filled) in [("secret-request-empty.png",false),("secret-request-masked.png",true),("question-request-empty.png",false),("question-request-typed.png",true)] {
                if name == "question-request-empty.png" {
                    state.update(cx,|s,cx| {
                        s.transcript = serde_json::from_value(serde_json::json!([{"id":"question-assistant","role":"assistant","parts":[{"kind":"input","id":"question-card","requestId":"ordinary-visual-request","questions":[{"id":"release-notes","header":"Release notes","question":"How should we prepare the release notes?","options":["Generate from merged pull requests","Write them manually"],"multiSelect":false,"multiline":false}],"resolved":false}],"createdAt":1,"deviceId":"fixture-device","status":"streaming"}])).unwrap();cx.notify();
                    });
                    pause(cx).await;
                }
                window.update(cx,|view,_,cx|{ if filled { view.composer.update(cx,|c,cx|if name.starts_with("secret") {c.fixture_private_secret_token(cx)} else {c.fixture_question_answer(cx)}); } cx.notify(); }).unwrap();
                pause(cx).await;
                gpui::AnyWindowHandle::from(window).update(cx,|_,w,_cx|{w.refresh();}).unwrap();
                pause(cx).await;
                let status = std::process::Command::new("python3")
                    .arg(concat!(env!("CARGO_MANIFEST_DIR"),"/examples/private-secrets-fixture/capture.py"))
                    .arg(std::process::id().to_string()).arg(output.join(name)).status().unwrap();
                assert!(status.success(),"native window capture failed");
            }
            drop(state);
            let _ = window.update(cx, |_, window, _| window.remove_window());
            pause(cx).await;
            cx.update(|cx|cx.quit());
        }).detach();
    });
    Ok(())
}
