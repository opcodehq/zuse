//! Native approval evidence with a scripted harness and temporary engine profile.
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, UserInputQuestion,
    permission::{RuleEffect, ToolApproval},
};
use zuse_providers::{Harness, HarnessError, RunControls};
use zuse_ui::*;
const CHAT: &str = "approval-demo";
struct ScriptedApproval;
#[async_trait]
impl Harness for ScriptedApproval {
    fn id(&self) -> HarnessId {
        HarnessId::ClaudeCode
    }
    fn display_name(&self) -> &str {
        "Approval capture fixture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![Model {
            id: "claude-sonnet-4-6".into(),
            label: "Sonnet 4.6".into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        }])
    }
    async fn run(
        &self,
        _: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let question = UserInputQuestion {
                id: "write-permissions".into(),
                header: "Approve file change".into(),
                question: "Allow writing src/permissions.rs?".into(),
                options: vec!["Yes".into(), "No".into()],
                multi_select: false,
                prefill: None,
                multiline: false,
                approval_prompt: None,
                approval: Some(ToolApproval::new(
                    "claude:Write",
                    serde_json::json!({"file_path":"src/permissions.rs", "content":"// Synthetic capture; no file is written.\n"}),
                    "Yes",
                    "No",
                )),
            };
            let answers = (controls.request_input)(vec![question])
                .await
                .unwrap_or_default();
            let allowed = answers
                .first()
                .is_some_and(|answer| answer.labels == ["Yes"]);
            let _ = tx
                .send(Ok(AgentEvent::Done {
                    status: if allowed {
                        DoneStatus::Completed
                    } else {
                        DoneStatus::Errored
                    },
                    result: None,
                    error: None,
                    session_id: None,
                }))
                .await;
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}
async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}
async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    path: PathBuf,
) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        window.refresh();
        let arena = window.draw(cx);
        window.present_if_needed();
        arena.clear();
    })?;
    pause(cx, 120).await;
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/private-secrets-fixture/capture.py"
        ))
        .arg(std::process::id().to_string())
        .arg(path)
        .status()?;
    anyhow::ensure!(status.success(), "native window capture failed");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let directory = temp.path().join("Fieldnotes");
    std::fs::create_dir(&directory)?;
    std::fs::create_dir(directory.join("src"))?;
    std::fs::write(
        directory.join("src/permissions.rs"),
        "// Isolated approval UI fixture.\n",
    )?;
    for args in [
        vec!["init", "--quiet", "-b", "main"],
        vec!["add", "src/permissions.rs"],
        vec![
            "-c",
            "user.name=UI Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--quiet",
            "-m",
            "Seed fixture workspace",
        ],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(&directory)
            .args(args)
            .status()?;
        anyhow::ensure!(
            status.success(),
            "could not prepare the temporary capture repository"
        );
    }
    let cwd = directory.canonicalize()?.to_string_lossy().into_owned();
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let registry = Arc::new(zuse_engine::HarnessRegistry::new());
    registry.register(Arc::new(ScriptedApproval));
    let core = zuse_engine::EngineCore::assemble(
        &temp.path().join("engine"),
        registry,
        HarnessId::ClaudeCode,
        None,
    )?;
    core.workspace.create_space(
        "fieldnotes",
        &core.device_id,
        &cwd,
        Some("Fieldnotes".into()),
        true,
    )?;
    let config = serde_json::from_value(
        serde_json::json!({"harness":"claude-code","model":"claude-sonnet-4-6","sandbox":"workspace-write"}),
    )?;
    core.workspace.create_chat(
        CHAT,
        Some("fieldnotes"),
        Some(&core.device_id),
        Some(config),
        Some(cwd.clone()),
    )?;
    core.workspace.rename_chat(CHAT, "Review approval rules")?;
    core.doc_host.open(CHAT)?;
    let (_, mut events) = core.sessions.subscribe(CHAT, 0)?;
    let (request_id, questions) = runtime.block_on(async {
        let request: RunRequest = serde_json::from_value(serde_json::json!({"cwd":cwd,"sandbox":"workspace-write","prompt":"Capture a synthetic approval"}))?;
        core.sessions.dispatch(CHAT, HarnessId::ClaudeCode, request, None).await?;
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), events.recv()).await??;
            if let AgentEvent::InputRequested {request_id, questions} = event.event { break Ok::<_,anyhow::Error>((request_id,questions)); }
        }
    })?;
    assert!(
        questions[0]
            .options
            .contains(&zuse_protocol::permission::ALLOW_PROJECT.into())
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let _ipc = runtime.block_on(zuse_engine::serve_ipc(port, core.rpc_service()))?;
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
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
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let chats = core.workspace.read_chats()?;
    let spaces = core.workspace.read_spaces()?;
    let device = core.device_id.clone();
    let rules = core.sessions.permission_rules();
    let project = "space:fieldnotes".to_owned();
    let at = chrono::Utc::now().timestamp_millis();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let mut prefs = settings::UiSettings::default(); prefs.github_star_banner_dismissed=true; settings::init(prefs.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(prefs.ui_font_family.clone(),prefs.ui_font_size,prefs.terminal_font_family.clone(),prefs.terminal_font_size,prefs.code_font_family.clone(),prefs.code_font_size,fonts,cx);
        theme_library::init(data.clone(),cx); appearance::init(appearance::AppearanceMode::Dark,prefs.theme_selection,prefs.accent,prefs.surface,cx); composer::init(cx,prefs.composer_send_behavior);
        history::init(prefs.git_history_columns,prefs.git_history_column_widths,prefs.git_history_column_order,prefs.git_history_author_display,cx);
        terminal::panel::init(cx);app_menus::init(cx);
        let state = cx.new(|_| {
            let mut state = state::AppState::new(); state.fixture_attachment_engine(handle);
            state.connection = zuse_protocol::view::ConnectionStatus::Ready;
            state.workspace_scope = Some(zuse_protocol::WorkspaceScope::Development);
            state.auth=Some(zuse_protocol::AuthState::SignedOut);
            state.apply_cloud_account(serde_json::json!({"user":null,"login":null}));
            state.local_device_id = Some(device.clone()); state.chats = chats; state.spaces=spaces; state.selected_space=Some("fieldnotes".into());
            state.devices=vec![serde_json::from_value(serde_json::json!({"id":device,"name":"This device","platform":"linux","lastSeenAt":null})).unwrap()];
            state.auto_selected=true;state.chats_synced=true;state.spaces_synced=true;
            state.selected_chat = Some(CHAT.into());
            state.transcript = serde_json::from_value(serde_json::json!([{"id":"approval","role":"assistant","parts":[{"kind":"input","id":"approval-card","requestId":request_id,"questions":questions,"resolved":false}],"createdAt":at,"deviceId":device,"status":"streaming"}])).unwrap();
            state
        });
        let window=cx.open_window(WindowOptions {window_background:theme::Theme::of(cx).window_background_appearance(),window_bounds:Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(0.),px(0.)),size(px(1200.),px(820.))))),..Default::default()},|_,cx|cx.new(|cx|shell::Shell::new(state.clone(),boot,cx))).unwrap();
        let controls=window.update(cx,|shell,_,_|shell.fixture_appshots_composer()).unwrap();
        state.update(cx,|_,cx|cx.notify()); cx.activate(true);
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                pause(cx,1000).await;
                state.update(cx,|state,cx| {state.receive_transcript_frame(zuse_doc::TranscriptFrame::Reset{reset:serde_json::from_value(serde_json::json!([
                    {"id":"request","role":"user","parts":[{"kind":"text","id":"request-text","text":"Please update the permission checks in src/permissions.rs."}],"createdAt":at,"deviceId":device},
                    {"id":"approval","role":"assistant","parts":[{"kind":"text","id":"summary","text":"I’ll update the permission checks. This requires permission to write the file."},{"kind":"input","id":"approval-card","requestId":request_id,"questions":questions,"resolved":false}],"createdAt":at+1,"deviceId":device,"status":"streaming"}
                ])).unwrap()},cx).unwrap();cx.notify();});
                pause(cx,700).await; capture(window.into(),cx,output.join("approval-prompt-dark.png")).await?;
                controls.update(cx,|composer,cx|composer.fixture_allow_project(cx));
                let mut snapshot = rules.snapshot(&project,CHAT)?;
                for _ in 0..40 { if !snapshot.rules.is_empty() {break;} pause(cx,100).await; snapshot = rules.snapshot(&project,CHAT)?; }
                anyhow::ensure!(snapshot.rules.len()==1 && snapshot.rules[0].chat_id.is_none(), "project grant was not saved through the production answer path");
                let approval = ToolApproval::new("claude:Write",snapshot.rules[0].input.clone(),"Yes","No");
                rules.save(&project,Some(CHAT),&approval)?;
                let chat_rule = rules.snapshot(&project,CHAT)?.rules.into_iter().find(|rule|rule.chat_id.is_some()).unwrap();
                rules.edit(&project,CHAT,&chat_rule.id,RuleEffect::Deny,true)?;
                anyhow::ensure!(rules.decide(&project,CHAT,SandboxLevel::WorkspaceWrite,&approval)?==Some(false),"chat denial must override the project grant");
                anyhow::ensure!(rules.decide(&project,CHAT,zuse_engine::permission_rules::RunPosture::PLAN,&approval)?==Some(false),"plan mode ceiling must deny");
                state.update(cx,|state,cx| {state.receive_transcript_frame(zuse_doc::TranscriptFrame::Reset{reset:serde_json::from_value(serde_json::json!([
                    {"id":"request","role":"user","parts":[{"kind":"text","id":"request-text","text":"Please update the permission checks in src/permissions.rs."}],"createdAt":at,"deviceId":device},
                    {"id":"response","role":"assistant","parts":[{"kind":"text","id":"response-text","text":"The project permission is saved. You can review or revoke it from the permissions inspector."}],"createdAt":at+1,"deviceId":device,"status":"complete"}
                ])).unwrap()},cx).unwrap(); cx.notify();});
                pause(cx,250).await;
                controls.update(cx,|composer,cx|composer.fixture_permission_inspector(cx));
                for _ in 0..40 {
                    if controls.update(cx, |composer, _| composer.fixture_permission_rule_count()) == Some(2) { break; }
                    pause(cx,100).await;
                }
                anyhow::ensure!(controls.update(cx, |composer, _| composer.fixture_permission_rule_count()) == Some(2), "inspector did not load its rules over RPC");
                pause(cx,700).await; capture(window.into(),cx,output.join("permission-inspector-dark.png")).await?;
                controls.update(cx,|composer,cx|composer.fixture_permission_controls(false,cx));
                pause(cx,200).await; capture(window.into(),cx,output.join("permission-inspector-edit.png")).await?;
                controls.update(cx,|composer,cx|composer.fixture_permission_controls(true,cx));
                pause(cx,200).await; capture(window.into(),cx,output.join("permission-inspector-revoke.png")).await?;
                controls.update(cx,|composer,cx|composer.fixture_permission_inspector(cx));
                for _ in 0..40 {
                    if controls.update(cx,|composer,_|composer.fixture_permission_rule_count()) == Some(2) { break; }
                    pause(cx,100).await;
                }
                anyhow::ensure!(controls.update(cx,|composer,_|composer.fixture_permission_rule_count()) == Some(2), "inspector did not refresh its rules");
                cx.update(|cx|appearance::set_mode(appearance::AppearanceMode::Light,cx));
                pause(cx,700).await; capture(window.into(),cx,output.join("permission-inspector-light.png")).await?;
                std::fs::write(output.join("result.json"),serde_json::to_vec_pretty(&serde_json::json!({"projectGrantSavedThroughComposer":true,"chatOverrideDenied":true,"readOnlyDenied":true,"rules":rules.snapshot(&project,CHAT)?.rules.len(),"syntheticHarness":true,"liveProvider":false,"editAndRevokeRendered":true}))?)?;
                Ok(())
            }.await;
            if let Err(error)=run {eprintln!("Approval capture failed: {error:#}"); *result.lock().unwrap()=Some(format!("{error:#}"));}
            drop(controls); drop(state);
            let _=gpui::AnyWindowHandle::from(window).update(cx,|_,window,cx| {window.disable_focus(); window.draw(cx).clear(); window.remove_window();}); pause(cx,100).await; cx.update(|cx|cx.quit());
        }).detach();
    });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
