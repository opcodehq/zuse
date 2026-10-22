//! Linux visual review fixture: the sidebar fixture's synthetic state, captured across
//! the main view and every settings page. ZUSE_REVIEW_ROUTES overrides the route list.
//! Sidebar options: ZUSE_SIDEBAR_COMPACT / ZUSE_SIDEBAR_HIDE_LABEL select layout;
//! ZUSE_SIDEBAR_BY_PROJECT groups by project, with a remote clone of the local repository;
//! ZUSE_SIDEBAR_COLLAPSED=<space ids> collapses those project groups and
//! ZUSE_SIDEBAR_FILTER=<space id> sets the project filter.
//! Route variants: `main:usage` seeds queued follow-ups and context/plan usage on the
//! worked conversation; `main:usage-open` also pins the composer's usage card open.
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use zuse_ui::*;

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let output = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "/tmp/zuse-review".into()),
    );
    std::fs::create_dir_all(&output)?;
    gpui_platform::application().with_assets(icons::Assets).with_http_client(fixture_http_client(runtime.handle().clone())).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_show_branch = true;
        settings.sidebar_compact = std::env::var_os("ZUSE_SIDEBAR_COMPACT").is_some();
        settings.sidebar_show_project_label = std::env::var_os("ZUSE_SIDEBAR_HIDE_LABEL").is_none();
        let by_project = std::env::var_os("ZUSE_SIDEBAR_BY_PROJECT").is_some();
        settings.sidebar_organization = if by_project { settings::SidebarOrganization::ByProject } else { settings::SidebarOrganization::InOneList };
        settings.sidebar_width = 310.0;
        settings.sidebar_pins_mut("local".into()).extend(["chat-0".into(), "chat-1".into()]);
        settings.space_filter = std::env::var("ZUSE_SIDEBAR_FILTER").ok();
        let project_path = data.join("fieldnotes");
        std::fs::create_dir_all(project_path.join("public")).unwrap();
        std::fs::write(project_path.join("public/favicon.svg"), r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect x="1" y="1" width="22" height="22" rx="6" fill="#668cf5"/><path d="M7 6h11v3h-8v3h6v3h-6v4H7Z" fill="white"/></svg>"##).unwrap();
        // ZUSE_SURFACE_THEME=1 keeps the theme's own (opaque) surface treatment.
        if std::env::var_os("ZUSE_SURFACE_THEME").is_none() { settings.surface = zuse_theme::SurfacePreference::Frosted; }
        settings.save(&data).unwrap();
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        appearance::init(if std::env::var_os("ZUSE_PALETTE_LIGHT").is_some() { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark }, settings.theme_selection, settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths,
            settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior); terminal::panel::init(cx); app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zuse_protocol::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
            if std::env::var_os("ZUSE_SIDEBAR_ACCOUNT").is_some() {
                s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Synced);
                s.auth = Some(zuse_protocol::AuthState::SignedIn {
                    user: zuse_protocol::UserProfile { id: "fixture-user".into(), email: "alex@example.test".into(), name: Some("Alex".into()) },
                    org_id: Some("fixture-org".into()),
                });
            }
            s.local_device_id = Some("local".into());
            s.devices = vec![serde_json::from_value(serde_json::json!({"id":"local","name":"This device","platform":std::env::consts::OS,"lastSeenAt":null})).unwrap()];
            s.selected_chat = Some("browser-fixture".into()); s.selected_space = Some("project".into());
            s.auto_selected = true; s.chats_synced = true; s.spaces_synced = true;
            s.spaces = vec![serde_json::from_value(serde_json::json!({"id":"project","deviceId":"local","path":project_path,"createdAt":"2026-09-08T00:00:00Z"})).unwrap()];
            s.chats = vec![serde_json::from_value(serde_json::json!({"id":"browser-fixture","deviceId":"local","spaceId":"project","title":"Build the Fieldnotes workspace","archived":false,"createdAt":"2026-09-08T00:00:00Z","config":{"harness":"claude-code","model":"claude-sonnet-4-6","reasoning":null,"sandbox":"workspace-write"}})).unwrap()];

            for (ix, title) in ["Polish the command palette", "Fix authentication redirects", "Add deployment status", "Review pull request comments", "Improve keyboard navigation", "Update project documentation", "Refine composer spacing", "Audit chat sync", "Build settings search"].iter().enumerate() {
                let mut chat = s.chats[0].clone();
                chat.id = format!("chat-{ix}");
                chat.title = Some((*title).into());
                chat.branch = Some(format!("fieldnotes/{}", title.to_lowercase().replace(' ', "-")));
                chat.source_context = Some(zuse_protocol::ConversationSourceContext {
                    checkout_id: "fixture-checkout".into(), repo_root: "/tmp/fieldnotes".into(),
                    cwd: "/tmp/fieldnotes".into(), branch: chat.branch.clone().unwrap(),
                    head_sha: None, observed_at: chrono::Utc::now(),
                });
                chat.created_at = chrono::Utc::now() - chrono::Duration::minutes((ix as i64 + 1) * 17);
                s.chats.push(chat);
            }
            s.devices.push(serde_json::from_value(serde_json::json!({"id":"remote","name":"Build server","platform":"linux","lastSeenAt":chrono::Utc::now()})).unwrap());
            s.spaces.push(serde_json::from_value(serde_json::json!({"id":"backend","deviceId":"remote","name":"API server","path":"/projects/backend","createdAt":chrono::Utc::now()})).unwrap());
            for ix in [2, 3, 5, 7, 8] {
                s.chats[ix].device_id = "remote".into();
                s.chats[ix].space_id = Some("backend".into());
            }
            // Each session works in its own checkout; some carry a diff.
            for (ix, (additions, deletions)) in [(1, (93, 20)), (2, (38_100, 1)), (3, (6, 1)), (5, (273, 4)), (7, (12, 0))] {
                let chat = &mut s.chats[ix];
                chat.checkout_id = Some(format!("fixture-checkout-{ix}"));
                let stat = state::CheckoutDiffStat { checkout_id: format!("fixture-checkout-{ix}"), device_id: chat.device_id.clone(), cwd: "/tmp/fieldnotes".into(), additions, deletions, files: 3 };
                s.checkout_diff_stats.push(stat);
            }
            for (ix, pr_state) in [(1, zuse_protocol::ChangeRequestState::Open), (2, zuse_protocol::ChangeRequestState::Merged), (3, zuse_protocol::ChangeRequestState::Closed), (4, zuse_protocol::ChangeRequestState::Open), (6, zuse_protocol::ChangeRequestState::Merged), (7, zuse_protocol::ChangeRequestState::Open), (8, zuse_protocol::ChangeRequestState::Closed)] {
                let chat = s.chats[ix].clone();
                let source = chat.source_context.as_ref().unwrap();
                s.fixture_sidebar_change_request(zuse_protocol::CheckoutChangeRequestStatus {
                    checkout_id: source.checkout_id.clone(), device_id: chat.device_id.clone(), cwd: source.repo_root.clone(), branch: source.branch.clone(), updated_at: chrono::Utc::now(),
                    change_request: Some(zuse_protocol::ChangeRequestSummary { provider: "github".into(), number: 412 + ix as u64, title: chat.title.clone().unwrap(), url: "https://github.com/opcodehq/zuse/pull/412".into(), state: pr_state, base_ref: "main".into(), head_ref: source.branch.clone() }),
                });
            }
            s.spaces[0].repository_id = Some("github.com/example/fieldnotes".into());
            if by_project {
                s.spaces.push(serde_json::from_value(serde_json::json!({"id":"fieldnotes-remote","deviceId":"remote","path":"/projects/fieldnotes","repositoryId":"github.com/example/fieldnotes","createdAt":chrono::Utc::now()})).unwrap());
                for ix in [3, 7] {
                    s.chats[ix].space_id = Some("fieldnotes-remote".into());
                }
            }
            s.chats[4].last_message_at = Some(chrono::Utc::now());
            s.chats[8].archived = true;
            // More projects: GitHub-hosted ones show their owner's avatar,
            // the rest a two-letter monogram; one chat is running.
            s.spaces[0].remote_repository = Some("github.com/rust-lang/fieldnotes".into());
            s.spaces[1].remote_repository = Some("github.com/tokio-rs/api-server".into());
            for (id, remote) in [("zuse", Some("github.com/opcodehq/zuse")), ("twitter-articles", None), ("storytelling-images", None), ("infraloop", Some("github.com/rust-lang/infraloop"))] {
                s.spaces.push(serde_json::from_value(serde_json::json!({"id":id,"deviceId":"local","path":data.join(id),"gitDetected":true,"remoteRepository":remote,"createdAt":"2026-09-01T00:00:00Z"})).unwrap());
            }
            for (id, space, title, minutes, diff) in [("zuse-publish", "zuse", "Let users publish their own plugins", 8, Some((5_500, 1_900))), ("zuse-new", "zuse", "New chat", 14 * 60, Some((38_100, 1))), ("articles-draft", "twitter-articles", "Draft the launch thread", 2 * 24 * 60, None)] {
                let mut chat = s.chats[0].clone();
                chat.id = id.into();
                chat.space_id = Some(space.into());
                chat.title = Some(title.into());
                chat.branch = None;
                chat.source_context = None;
                chat.created_at = chrono::Utc::now() - chrono::Duration::minutes(minutes);
                if let Some((additions, deletions)) = diff {
                    chat.checkout_id = Some(format!("fixture-checkout-{id}"));
                    s.checkout_diff_stats.push(state::CheckoutDiffStat { checkout_id: format!("fixture-checkout-{id}"), device_id: "local".into(), cwd: "/tmp/fieldnotes".into(), additions, deletions, files: 4 });
                }
                s.chats.push(chat);
            }
            // The open conversation works on a branch with uncommitted
            // changes: the top bar's branch name and "· N" count.
            s.chats[0].source_context = Some(zuse_protocol::ConversationSourceContext {
                checkout_id: "fixture-checkout".into(), repo_root: "/tmp/fieldnotes".into(),
                cwd: "/tmp/fieldnotes".into(), branch: "alex/connection-handoff".into(),
                head_sha: None, observed_at: chrono::Utc::now(),
            });
            s.chats[0].branch = Some("alex/connection-handoff".into());
            s.chats[0].checkout_id = Some("fixture-checkout".into());
            s.chats[0].cwd = Some("/tmp/fieldnotes".into());
            s.checkout_diff_stats.push(state::CheckoutDiffStat { checkout_id: "fixture-checkout".into(), device_id: "local".into(), cwd: "/tmp/fieldnotes".into(), additions: 214, deletions: 37, files: 10 });
            // Every Threads row state: running, waiting on input, failed
            // unseen and finished unseen (chat-3, bold).
            s.chats[7].last_message_at = Some(chrono::Utc::now());
            let session = |chat_id: &str, device_id: &str, status| zuse_protocol::Session { last_completed_turn: None, running_subagents: 0, chat_id: chat_id.into(), device_id: device_id.into(), status, started_at: Some(chrono::Utc::now()), updated_at: chrono::Utc::now() };
            s.apply_sessions(vec![
                session("zuse-publish", "local", zuse_protocol::SessionStatus::Working),
                session("chat-5", "local", zuse_protocol::SessionStatus::AwaitingInput),
                session("chat-6", "remote", zuse_protocol::SessionStatus::Errored),
            ]);
            s
        });
        let boot = EngineBootConfig { target: None,  data_dir: data, ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::ClaudeCode };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(12.),px(30.)), size(px(1100.),px(800.))))),
            titlebar: Some(gpui::TitlebarOptions { title: None, appears_transparent: true, traffic_light_position: Some(gpui::point(px(14.),px(14.))) }),
            app_owns_titlebar_drag: true,
            // Same backdrop as the app window, or the frosted surface has nothing to blur.
            window_background: theme::Theme::of(cx).window_background_appearance(),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        if let Ok(ids) = std::env::var("ZUSE_SIDEBAR_COLLAPSED") {
            let _ = window.update(cx, |shell, _, cx| shell.fixture_collapse_sidebar_groups(&ids.split(',').collect::<Vec<_>>(), cx));
        }
        appearance::reapply_window_background(cx);
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        let routes: Vec<String> = std::env::var("ZUSE_REVIEW_ROUTES")
            .unwrap_or_else(|_| "main,settings/general,settings/providers,settings/defaults,settings/plugins,settings/devices,settings/appearance,settings/notifications,settings/voice,settings/shortcuts,settings/files,settings/archived".into())
            .split(',').map(str::to_owned).collect();
        cx.spawn(async move |cx| {
            pause(cx, 1500).await;
            for route in routes {
                let _ = window.update(cx, |shell, window, cx| {
                    shell.fixture_review_route(&route, cx);
                    if route.ends_with("model-picker") { shell.fixture_open_model_picker(window, cx); } else if route.ends_with("-menu") { shell.fixture_open_menu(&route, window, cx); } else if route.starts_with("panel/") || route.starts_with("main/") { shell.fixture_open_panel(&route, window, cx); }
                });
                pause(cx, 700).await;
                let name = route.replace('/', "-");
                if let Err(error) = capture_frame(window, &output, &name, cx).await {
                    eprintln!("capture {route} failed: {error:#}");
                }
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
    Ok(())
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

// Xvfb's frame loop can stay idle; present the current scene before reading pixels.
async fn capture_frame(
    window: gpui::WindowHandle<shell::Shell>,
    directory: &std::path::Path,
    name: &str,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    gpui::AnyWindowHandle::from(window).update(cx, |_, w, cx| {
        w.refresh();
        let arena = w.draw(cx);
        w.present_if_needed();
        arena.clear();
    })?;
    pause(cx, 120).await;
    let windows = std::process::Command::new("xdotool")
        .args([
            "search",
            "--onlyvisible",
            "--pid",
            &std::process::id().to_string(),
        ])
        .output()?;
    let id = String::from_utf8(windows.stdout)?
        .lines()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("window not visible"))?;
    let status = std::process::Command::new("import")
        .args(["-window", &id])
        .arg(directory.join(format!("{name}.png")))
        .status()?;
    anyhow::ensure!(status.success(), "import failed");
    Ok(())
}
