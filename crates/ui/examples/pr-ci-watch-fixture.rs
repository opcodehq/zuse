//! Actual native Shell/PR watch captures over isolated, deterministic RPC.
//! No agent, GitHub request, working-tree mutation or Git push is executed.
use async_trait::async_trait;
use futures::StreamExt;
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
use zuse_protocol::{change_request_assessment::CiState, pr_watch::PrWatchStatus};
use zuse_rpc::{RpcError, RpcReply, RpcService, methods};
use zuse_ui::*;

const CHAT: &str = "ci-watch-fixture";
struct CaptureRpc {
    statuses: watch::Sender<Vec<PrWatchStatus>>,
    calls: Mutex<Vec<String>>,
}
impl CaptureRpc {
    fn phase(&self, auto_fix: bool, attempts: usize, exhausted: bool) {
        self.statuses.send_modify(|rows| {
            for row in rows {
                row.auto_fix = auto_fix;
                row.attempts = attempts;
                row.ci = CiState::Failed;
                row.error = exhausted.then(|| {
                    "Paused after three repair attempts. Review changes before restarting.".into()
                });
            }
        });
    }
}
#[async_trait]
impl RpcService for CaptureRpc {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        self.calls.lock().unwrap().push(method.into());
        match method {
            methods::ENGINE_INFO => RpcReply::value(&zuse_protocol::EngineInfo {
                device_id: "local".into(),
                workspace_scope: zuse_protocol::WorkspaceScope::Development,
                cursor_sdk_version: None,
                capabilities: vec![zuse_protocol::capabilities::PR_WATCH_V1.into()],
            }),
            methods::ENGINE_READY => RpcReply::value(&Value::Null),
            methods::OBSERVE_PR_WATCH => {
                let chat_id = params["chatId"].as_str().unwrap().to_owned();
                let url = params["url"].as_str().unwrap().to_owned();
                self.statuses.send_if_modified(|rows| {
                    if rows.iter().any(|r| r.url == url && r.chat_id == chat_id) {
                        return false;
                    }
                    rows.push(PrWatchStatus {
                        chat_id,
                        url,
                        branch: "feat/session-refresh".into(),
                        enabled: true,
                        auto_fix: false,
                        attempts: 0,
                        max_attempts: 3,
                        ci: CiState::Failed,
                        failure_sequence: 1,
                        error: None,
                        repairing: false,
                        can_auto_fix: true,
                    });
                    true
                });
                let rows = self.statuses.borrow();
                RpcReply::value(
                    rows.iter()
                        .find(|r| r.url == params["url"].as_str().unwrap())
                        .unwrap(),
                )
            }
            methods::WATCH_PR_WATCHES => {
                let stream = futures::stream::unfold(
                    (self.statuses.subscribe(), true),
                    |(mut rx, first)| async move {
                        if !first && rx.changed().await.is_err() {
                            return None;
                        }
                        let value = serde_json::to_value(rx.borrow_and_update().clone()).unwrap();
                        Some((value, (rx, false)))
                    },
                )
                .boxed();
                Ok(RpcReply::Stream(stream))
            }
            methods::SET_PR_WATCH => {
                self.statuses.send_modify(|rows| {
                    for row in rows {
                        if row.chat_id == params["chatId"].as_str().unwrap()
                            && row.url == params["url"].as_str().unwrap()
                        {
                            row.enabled = params["enabled"].as_bool().unwrap();
                            row.auto_fix = row.enabled && params["autoFix"].as_bool().unwrap();
                            if params["resetAttempts"].as_bool().unwrap_or(false) {
                                row.attempts = 0;
                            }
                        }
                    }
                });
                let rows = self.statuses.borrow();
                RpcReply::value(
                    rows.iter()
                        .find(|r| r.url == params["url"].as_str().unwrap())
                        .unwrap(),
                )
            }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
}
async fn pause(cx: &mut AsyncApp, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}
async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.refresh();
        let arena = w.draw(cx);
        w.present_if_needed();
        arena.clear();
    })?;
    pause(cx, 150).await;
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/private-secrets-fixture/capture.py"
        ))
        .arg(std::process::id().to_string())
        .arg(output.join(format!("{name}.png")))
        .status()?;
    anyhow::ensure!(status.success(), "native capture failed");
    Ok(())
}
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let rpc = Arc::new(CaptureRpc {
        statuses: watch::channel(vec![]).0,
        calls: Mutex::new(vec![]),
    });
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let _ipc = runtime.block_on(zuse_engine::serve_ipc(port, rpc.clone()))?;
    let data = temp.path().join("ui");
    std::fs::create_dir_all(&data)?;
    let boot = EngineBootConfig {
        target: None,
        data_dir: data.clone(),
        ipc_port: port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::ClaudeCode,
    };
    let engine = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let failure = Arc::new(Mutex::new(None));
    let failed = failure.clone();
    let capture_rpc = rpc.clone();
    let results = output.clone();
    gpui_platform::application().with_assets(icons::Assets).with_http_client(fixture_http_client(runtime.handle().clone())).run(move |cx| {
        gpui_tokio::init(cx);gpui_base::init(cx);
        let mut settings=settings::UiSettings::default();
        settings.sidebar_width=260.;settings.surface=zuse_theme::SurfacePreference::Frosted;
        settings.notifications_enabled=false;settings.sound_attention_enabled=false;
        settings::init(settings.clone(),data.clone(),cx);
        let fonts=typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(),settings.ui_font_size,settings.terminal_font_family.clone(),settings.terminal_font_size,settings.code_font_family.clone(),settings.code_font_size,fonts,cx);
        theme_library::init(data.clone(),cx);appearance::init(appearance::AppearanceMode::Dark,settings.theme_selection,settings.accent,settings.surface,cx);
        history::init(settings.git_history_columns,settings.git_history_column_widths,settings.git_history_column_order,settings.git_history_author_display,cx);
        composer::init(cx,settings.composer_send_behavior);terminal::panel::init(cx);app_menus::init(cx);
        cx.set_global(pull_request_card::PullRequestWatchFixture);
        let state=cx.new(|_| {
            let mut s=state::AppState::new();s.fixture_attachment_engine(engine);
            s.connection=zuse_protocol::view::ConnectionStatus::Ready;s.workspace_scope=Some(zuse_protocol::WorkspaceScope::Development);
            s.local_device_id=Some("local".into());s.selected_chat=Some(CHAT.into());s.selected_space=Some("project".into());
            s.devices=vec![serde_json::from_value(json!({"id":"local","name":"This device","platform":"linux","lastSeenAt":null})).unwrap()];
            s.spaces=vec![serde_json::from_value(json!({"id":"project","deviceId":"local","name":"Fieldnotes","path":data.join("Fieldnotes"),"createdAt":chrono::Utc::now()})).unwrap()];
            s.chats=vec![serde_json::from_value(json!({"id":CHAT,"deviceId":"local","spaceId":"project","title":"Watch pull request CI","cwd":data.join("Fieldnotes"),"branch":"feat/session-refresh","archived":false,"createdAt":chrono::Utc::now(),"config":{"harness":"claude-code","model":"claude-sonnet-4-6","sandbox":"workspace-write"}})).unwrap()];
            s.auto_selected=true;s.chats_synced=true;s.spaces_synced=true;s
        });
        let window=cx.open_window(WindowOptions {window_bounds:Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(12.),px(20.)),size(px(1240.),px(920.))))),window_background:theme::Theme::of(cx).window_background_appearance(),..Default::default()},|_,cx|cx.new(|cx|shell::Shell::new(state.clone(),boot,cx))).unwrap();
        cx.activate(true);state.update(cx,|_,cx|cx.notify());
        cx.spawn(async move |cx| {
            let result:anyhow::Result<()>=async {
                pause(cx,500).await;
                window.update(cx,|shell,_,cx|shell.fixture_review_route("main:prcard-flow-failed",cx))?;
                pause(cx,1200).await;
                anyhow::ensure!(!capture_rpc.statuses.borrow().is_empty(),"production watch control did not subscribe");
                capture(window.into(),cx,&output,"pr-ci-watch-watching").await?;
                capture_rpc.phase(true,1,false);pause(cx,500).await;
                capture(window.into(),cx,&output,"pr-ci-watch-auto-fix").await?;
                capture_rpc.phase(false,3,true);pause(cx,500).await;
                capture(window.into(),cx,&output,"pr-ci-watch-exhausted").await?;
                cx.update(|cx|appearance::set_mode(appearance::AppearanceMode::Light,cx));pause(cx,500).await;
                capture(window.into(),cx,&output,"pr-ci-watch-exhausted-light").await?;
                Ok(())
            }.await;
            if let Err(e)=result {*failed.lock().unwrap()=Some(format!("{e:#}"));}
            let _=gpui::AnyWindowHandle::from(window).update(cx,|_,window,cx| {
                window.disable_focus();
                window.draw(cx).clear();
                window.remove_window();
            });
            pause(cx,100).await;
            cx.update(|cx|cx.quit());
        }).detach();
    });
    std::fs::write(
        results.join("rpc-calls.json"),
        serde_json::to_vec_pretty(&*rpc.calls.lock().unwrap())?,
    )?;
    if let Some(message) = failure.lock().unwrap().take() {
        anyhow::bail!(message);
    }
    Ok(())
}
