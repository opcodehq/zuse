//! Render the production chat/pane with explicitly synthetic device data.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zuse_ui::*;
async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let capture_window = window.update(cx, |_, window, _| -> anyhow::Result<String> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match HasWindowHandle::window_handle(&*window)?.as_raw() {
            RawWindowHandle::Xcb(handle) => Ok(handle.window.get().to_string()),
            RawWindowHandle::Xlib(handle) => Ok(handle.window.to_string()),
            _ => anyhow::bail!("Linux fixture capture requires an X11 display"),
        }
    })??;
    // Static native fixtures must submit their freshly drawn scene explicitly.
    window.update(cx, |_, window, cx| {
        window.refresh();
        let arena = window.draw(cx);
        window.present_if_needed();
        arena.clear();
    })?;
    cx.background_executor()
        .timer(Duration::from_millis(50))
        .await;
    #[cfg(target_os = "linux")]
    {
        use anyhow::Context as _;
        let status = std::process::Command::new("import")
            .args(["-window", &capture_window])
            .arg(path)
            .status()
            .context("Install ImageMagick for X11 fixture screenshots")?;
        anyhow::ensure!(status.success(), "X11 fixture screenshot failed");
        let screenshot = image::open(path)?.to_rgba8();
        let first = screenshot.get_pixel(0, 0);
        anyhow::ensure!(
            screenshot.pixels().any(|pixel| pixel != first),
            "X11 fixture capture was blank"
        );
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        window.update(cx, |_, window, _| {
            window.render_to_image()?.save(path)?;
            Ok(())
        })?
    }
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    if output.join("result.txt").exists() {
        std::fs::remove_file(output.join("result.txt"))?;
    }
    let root = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zuse_engine::EngineCore::assemble_with_profile(
            zuse_engine::EngineProfile::local(root.path()).unwrap(),
            Arc::new(zuse_engine::default_registry()),
            zuse_protocol::HarnessId::Codex,
            None,
        )
    })?;
    core.workspace
        .create_chat("simulator-fixture", None, Some(&core.device_id), None, None)?;
    core.workspace
        .rename_chat("simulator-fixture", "Check the mobile app")?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let _ipc = runtime.block_on(zuse_engine::serve_ipc(port, core.rpc_service()))?;
    let boot = EngineBootConfig {
        target: None,
        data_dir: root.path().join("ui"),
        ipc_port: port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zuse_protocol::HarnessId::Codex,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let setup_handle=handle.clone();
    let chats = core.workspace.read_chats()?;
    let device = core.device_id.clone();
    let fail = Arc::new(std::sync::Mutex::new(None));
    let result = fail.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);gpui_base::init(cx);
        let settings=settings::UiSettings::default();settings::init(settings.clone(),boot.data_dir.clone(),cx);
        let fonts=typography::register_fonts(cx);typography::init(settings.ui_font_family.clone(),settings.ui_font_size,settings.terminal_font_family.clone(),settings.terminal_font_size,settings.code_font_family.clone(),settings.code_font_size,fonts,cx);
        theme_library::init(boot.data_dir.clone(),cx);appearance::init(appearance::AppearanceMode::Dark,settings.theme_selection,settings.accent,settings.surface,cx);
        history::init(settings.git_history_columns,settings.git_history_column_widths,settings.git_history_column_order,settings.git_history_author_display,cx);
        composer::init(cx,settings.composer_send_behavior);terminal::panel::init(cx);app_menus::init(cx);
        let app=cx.new(|_| {let mut s=state::AppState::new();s.fixture_simulator_engine(handle);s.connection=zuse_protocol::view::ConnectionStatus::Ready;s.workspace_scope=Some(zuse_protocol::WorkspaceScope::Development);s.local_device_id=Some(device.clone());s.chats=chats;s.selected_chat=Some("simulator-fixture".into());s.auto_selected=true;s.chats_synced=true;s.spaces_synced=true;s});
        let window=cx.open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(20.),px(40.)),size(px(1280.),px(900.))))),..Default::default()},|_,cx|cx.new(|cx|shell::Shell::new(app.clone(),boot,cx))).unwrap();cx.activate(true);
        cx.spawn(async move |cx| {
            let run:anyhow::Result<()>=async {
                cx.background_executor().timer(Duration::from_millis(1200)).await;
                app.update(cx,|state,cx| {
                    state.receive_transcript_frame(zuse_doc::TranscriptFrame::Reset{reset:serde_json::from_value(serde_json::json!([
                        {"id":"fixture-user","role":"user","parts":[{"id":"text","kind":"text","text":"Check the mobile layout, then test the Home button."}],"createdAt":1788900000000_i64,"deviceId":device},
                        {"id":"fixture-assistant","role":"assistant","parts":[{"id":"text","kind":"text","text":"I’ll open a device beside this chat and inspect the screen."}],"createdAt":1788900001000_i64,"deviceId":device,"status":"complete"}
                    ])).unwrap()},cx).unwrap();cx.notify();
                });
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let requirements=vec![zuse_protocol::simulators::Requirement{platform:zuse_protocol::simulators::Platform::Ios,ready:false,message:"iOS Simulator requires a Mac with Xcode".into()},zuse_protocol::simulators::Requirement{platform:zuse_protocol::simulators::Platform::Android,ready:false,message:"Install Android SDK Platform-Tools and Emulator, then create an AVD in Android Studio".into()}];
                for (name,state) in [("simulator-setup",zuse_protocol::simulators::State{requirements:requirements.clone(),..Default::default()}),("simulator-picker",zuse_protocol::simulators::State{enabled:true,devices:vec![zuse_protocol::simulators::Device{id:"ios:fixture".into(),name:"iPhone 17 Pro".into(),platform:zuse_protocol::simulators::Platform::Ios,runtime:"iOS 26.0".into(),booted:false,serial:None},zuse_protocol::simulators::Device{id:"android:Fixture_Pixel".into(),name:"Pixel 9".into(),platform:zuse_protocol::simulators::Platform::Android,runtime:"Android 16".into(),booted:true,serial:Some("emulator-5554".into())},zuse_protocol::simulators::Device{id:"android:Fixture_Pixel_Tablet".into(),name:"Pixel Tablet".into(),platform:zuse_protocol::simulators::Platform::Android,runtime:"Android 16".into(),booted:false,serial:None}],..Default::default()})] {
                    window.update(cx,|shell,window,cx|shell.fixture_simulator(state,None,window,cx))?;
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                    capture(window.into(),cx,&output.join(format!("{name}.png"))).await?;
                }
                use zuse_protocol::simulators::{Setup, SetupStatus, AgentStatus, SetupRequirement};
                let host_requirements=vec![
                    SetupRequirement{label:"Node.js".into(),ready:true,detail:"v24.22.1".into()},
                    SetupRequirement{label:"npm".into(),ready:true,detail:"Available".into()},
                    SetupRequirement{label:"Device platform".into(),ready:true,detail:"Xcode and Android SDK tools available".into()},
                ];
                let setup_window=cx.update(|cx|cx.open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(30.),px(30.)),size(px(400.),px(720.))))),..Default::default()},|window,cx|cx.new(|cx|simulator::SimulatorPane::new(setup_handle.clone(),"simulator-fixture".into(),device.clone(),window,cx))))?;
                for (name,status,step,agent_status,detail) in [
                    ("simulator-onboarding",SetupStatus::Disabled,0,AgentStatus::Off,""),
                    ("simulator-checking",SetupStatus::Checking,0,AgentStatus::Off,"Checking this host…"),
                    ("simulator-installing-hub",SetupStatus::Installing,1,AgentStatus::Off,"Installing Device Hub 0.15.3…"),
                    ("simulator-starting-hub",SetupStatus::Starting,2,AgentStatus::Off,"Starting Device Hub…"),
                    ("simulator-discovering",SetupStatus::Discovering,3,AgentStatus::Off,"Finding available devices…"),
                    ("simulator-installing-agent",SetupStatus::Ready,4,AgentStatus::Installing,"Device Hub is ready"),
                    ("simulator-install-error",SetupStatus::Error,1,AgentStatus::Off,"Device setup failed: npm could not reach the package registry. Check your connection and try again."),
                    ("simulator-agent-error",SetupStatus::Ready,4,AgentStatus::Error,"Device Hub is ready"),
                ] {
                    let setup=Setup{status,step,agent_status,detail:detail.into(),requirements:host_requirements.clone(),agent_detail:if agent_status==AgentStatus::Error {"Agent tool setup failed: package verification failed. Try again to download a fresh copy.".into()} else {"Installing agent-device 0.21.23…".into()},..Default::default()};
                    setup_window.update(cx,|pane,_,cx|pane.fixture_load(zuse_protocol::simulators::State{enabled:status!=SetupStatus::Disabled,agent_access:true,setup:Some(setup),..Default::default()},None,cx))?;
                    cx.background_executor().timer(Duration::from_millis(350)).await;
                    capture(setup_window.into(),cx,&output.join(format!("{name}.png"))).await?;
                }
                // Keep the setup controls inside a narrow right pane as descriptions wrap.
                let narrow_window=cx.update(|cx|cx.open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(30.),px(30.)),size(px(280.),px(720.))))),..Default::default()},|window,cx|cx.new(|cx|simulator::SimulatorPane::new(setup_handle.clone(),"simulator-fixture".into(),device.clone(),window,cx))))?;
                narrow_window.update(cx,|pane,_,cx|pane.fixture_load(zuse_protocol::simulators::State{agent_access:true,setup:Some(Setup{status:SetupStatus::Disabled,requirements:host_requirements.clone(),..Default::default()}),..Default::default()},None,cx))?;
                cx.background_executor().timer(Duration::from_millis(350)).await;
                capture(narrow_window.into(),cx,&output.join("simulator-onboarding-narrow.png")).await?;
                narrow_window.update(cx,|_,window,_|window.remove_window())?;
                setup_window.update(cx,|_,window,_|window.remove_window())?;
                let pixels=image::load_from_memory(include_bytes!("simulator-sample.png"))?.to_rgba8();
                let mut png=std::io::Cursor::new(Vec::new());image::DynamicImage::ImageRgba8(pixels).write_to(&mut png,image::ImageFormat::Png)?;
                use base64::Engine as _;
                let frame=zuse_protocol::simulators::Frame{data:base64::engine::general_purpose::STANDARD.encode(png.into_inner()),mime_type:"image/png".into(),width:390,height:844};
                let device=zuse_protocol::simulators::Device{id:"android:Fixture_Pixel".into(),name:"Pixel 9".into(),platform:zuse_protocol::simulators::Platform::Android,runtime:"Android 16".into(),booted:true,serial:Some("emulator-5554".into())};
                let state=zuse_protocol::simulators::State{enabled:true,agent_access:true,devices:vec![device.clone()],sessions:vec![device],..Default::default()};
                window.update(cx,|shell,w,cx|shell.fixture_simulator(state,Some(frame),w,cx))?;
                for (name,mode) in [("simulator-viewer-dark",appearance::AppearanceMode::Dark),("simulator-viewer-light",appearance::AppearanceMode::Light)] {
                    cx.update(|cx|appearance::set_mode(mode,cx));
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                    capture(window.into(),cx,&output.join(format!("{name}.png"))).await?;
                    if name == "simulator-viewer-dark" {
                        // Exercise the actual toolbar hit target, then dismiss in the letterbox.
                        for (position, capture_menu) in [(gpui::point(px(1261.),px(55.)),true),(gpui::point(px(800.),px(300.)),false)] {
                            gpui::AnyWindowHandle::from(window).update(cx,|_,w,cx| {
                                w.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent{position,button:gpui::MouseButton::Left,click_count:1,..Default::default()}),cx);
                                w.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent{position,button:gpui::MouseButton::Left,click_count:1,..Default::default()}),cx);
                            })?;
                            cx.background_executor().timer(Duration::from_millis(200)).await;
                            if capture_menu {capture(window.into(),cx,&output.join("simulator-menu.png")).await?;}
                        }
                    }
                }
                std::fs::write(output.join("result.txt"),"Production Shell and SimulatorPane with scripted onboarding, helper installation/progress/failure, picker and viewer states. No real device execution.\n")?;Ok(())
            }.await;
            if let Err(error)=run {*result.lock().unwrap()=Some(format!("{error:#}"));}
            let _=window.update(cx,|_,w,_|w.remove_window());cx.update(|cx|cx.quit());
        }).detach();
    });
    runtime.block_on(core.shutdown());
    if let Some(error) = fail.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
